use crate::*;

/// Values computed earlier in the loop iteration that the tick consumes.
pub(crate) struct TickInputs {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    pub(crate) viewport_size: (usize, usize),
    pub(crate) stub_animation_active: bool,
    pub(crate) frame_interval: Duration,
    pub(crate) sdf_animation_active: bool,
    pub(crate) playing_now: bool,
}

pub(crate) enum TickFlow {
    /// Move on to the next loop iteration.
    Continue,
    /// The editor requested shutdown; leave the event loop.
    Quit,
}

fn capture_param_sync_revision(
    app: &app::App,
    ctx: &LoopCtx<'_>,
    track: usize,
    selected_neural_neurons: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> ParamSyncRevision {
    let mut selected_steps = ctx
        .shared
        .selected_steps
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    selected_steps.sort_unstable();
    ParamSyncRevision {
        track,
        scene: ctx.shared.state.current_scene_index(),
        pattern_epoch: ctx
            .shared
            .state
            .transport
            .pattern_epoch
            .load(Ordering::Relaxed),
        song_row_mirror_epoch: app.song_row_mirror_epoch,
        ui_epoch: ctx.shared.ui_epoch.load(Ordering::Relaxed),
        fx_epoch: ctx.shared.fx_epoch.load(Ordering::Relaxed),
        sound_binding_epoch: app.sound_binding_epoch,
        display_step: displayed_plock_step(
            &ctx.shared.state,
            track,
            selected_steps.first().copied(),
        ),
        selected_steps,
        selected_neural_neurons: selected_neural_neurons.iter().copied().collect(),
    }
}

pub(super) fn claim_param_sync_revision(
    previous: &mut Option<ParamSyncRevision>,
    revision: &ParamSyncRevision,
) -> bool {
    if previous.as_ref() == Some(revision) {
        return false;
    }
    *previous = Some(revision.clone());
    true
}

fn sync_track_params_delta(
    previous: &mut Option<ParamSyncRevision>,
    revision: ParamSyncRevision,
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) {
    if !claim_param_sync_revision(previous, &revision) {
        return;
    }
    sync_track_params_with_neural_selection(
        rt,
        app,
        state,
        track,
        selected_steps,
        Some(selected_neural_neurons),
    );
}

fn sync_fx_param_bindings_delta(
    previous: &mut Option<ParamSyncRevision>,
    revision: ParamSyncRevision,
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> bool {
    if !claim_param_sync_revision(previous, &revision) {
        return false;
    }
    let dirty = sync_fx_param_binding_fields_with_neural_selection(
        rt,
        app,
        state,
        track,
        selected_steps,
        Some(selected_neural_neurons),
    );
    dirty
}

/// The slice of fx panel publication that buffers other than `*fx*` read:
/// `*samples*` and `*macro-mappings*` read `SEQ.instrument-panel`
/// (content/ui/browser.lisp `rack-panel-open?`, content/ui/macros.lisp), and
/// `*metal*` reads `SEQ.step-has-plocks` (content/ui/step-grid.lisp). These
/// must stay current for the selected track even while `*fx*` is hidden.
pub(super) fn sync_shared_panel_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: Option<usize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    structural: bool,
) {
    let instrument_panel = track.map_or_else(
        || Value::List(vec![]),
        |track| build_instrument_panel_value(app, track, selected_steps),
    );
    if structural {
        rt.set_reactive("SEQ", "instrument-panel", instrument_panel);
    } else {
        rt.set_reactive_value_patch("SEQ", "instrument-panel", instrument_panel);
    }
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        track.map_or_else(
            || Value::List(vec![]),
            |track| build_step_has_plocks(state, track, &app.graph.effect_descriptors),
        ),
    );
}

pub(super) fn sync_fx_panel_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: Option<usize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    structural: bool,
) {
    let publish = |rt: &mut Runtime, field: &str, value: Value| {
        if structural {
            rt.set_reactive("SEQ", field, value);
        } else {
            rt.set_reactive_value_patch("SEQ", field, value);
        }
    };
    publish(
        rt,
        "effects",
        track.map_or_else(
            || Value::List(vec![]),
            |track| build_effects_value(state, track, &app.graph.effect_descriptors, selected_steps),
        ),
    );
    publish(
        rt,
        "midi-effects",
        track.map_or_else(
            || Value::List(vec![]),
            |track| build_midi_effects_value(state, track, selected_steps),
        ),
    );
    sync_shared_panel_state(rt, app, state, track, selected_steps, structural);
    publish(
        rt,
        "bus-effects",
        build_bus_effects_value_for_selection(app, Some(selected_steps)),
    );
}

/// Post-event reactive sync + render: diffs sequencer/transport state against
/// the previous frame, republishes reactives, and renders when dirty.
#[allow(clippy::too_many_lines)]
pub(crate) fn reactive_tick_and_render(
    mut app: &mut app::App,
    mut editor: &mut Editor,
    backend: &mut AppBackend,
    ctx: &mut LoopCtx<'_>,
    inputs: TickInputs,
    last_render_at: &mut Instant,
    stub_animation_cache: &mut StubAnimationRenderCache,
    ui_loop_stats: &mut UiLoopStats,
) -> Result<TickFlow, Box<dyn std::error::Error>> {
    poll_pending_compile_status(
        &mut app,
        &mut editor,
        &ctx.shared.state,
        &ctx.shared.current_track,
        &ctx.shared.selected_steps,
        &ctx.shared.fx_epoch,
        &ctx.shared.ui_epoch,
    );

    // Inline runtime values are polled while building a render frame. The UI
    // is otherwise event-driven, so scheduler-owned channel changes must ask
    // for a frame or a process-driven slider remains still until unrelated
    // input redraws it (and appears to snap under the pointer).
    let process_channel_values_version =
        ctx.shared.state.process_channel_values_version();
    if process_channel_values_version != ctx.frame.prev_process_channel_values_version {
        ctx.frame.prev_process_channel_values_version = process_channel_values_version;
        editor.mark_needs_redraw();
    }

    // Keep plugin-delay-compensation pads in sync with whatever mutated the
    // effect chains this frame (installs, undo/redo, project load, scenes).
    // Change-detecting: writes to the graph only when the pad set differs.
    app.refresh_latency_compensation();

    // Live note-on stamps (bead eseq-2awi): reposition held live-note targets
    // onto the render-timeline beat the audio callback actually sounded them
    // at, ahead of the release that writes them into the pattern.
    apply_live_trigger_stamps(&ctx.shared.state, &ctx.shared.held_notes);

    // Rolled-hit recording (docs/rolling-core-spec.md 6): drain the
    // scheduler's per-hit feedback and flush released keys' batches into the
    // pattern (or the pending take), then republish the step grid exactly
    // like a live-key recording.
    let (roll_recorded, roll_take_recorded) = tick_roll_record(&mut app, ctx.shared);
    if roll_take_recorded {
        app.mark_recording_take_changed();
        editor.mark_needs_redraw();
    }
    if roll_recorded {
        app.mark_recording_take_changed();
        let ct = ctx.shared.current_track.load(Ordering::Relaxed);
        let rt = editor.runtime_mut();
        rt.set_reactive("SEQ", "steps", build_steps_value(&ctx.shared.state, ct));
        sync_all_track_sequencer_state(rt, &ctx.shared.state, &app, ct, &ctx.shared.selected_steps);
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
        editor.mark_needs_redraw();
    }

    // Live step-param printing (bead eseq-jc9): while playing+recording, an
    // armed *step*-buffer param latch writes onto each trigger step the
    // playhead passes. The tick pushes targeted Step invalidations (drained
    // below in this same frame) instead of bumping ui_epoch; the writes ride
    // the open "Record take" undo transaction like live note recording.
    let step_print_tick = tick_step_print(ctx.shared, editor.runtime_mut());
    if step_print_tick.printed {
        app.mark_recording_take_changed();
    }
    if step_print_tick.display_dirty {
        // The picker readouts moved to (or back from) the print latch: flush
        // them through the same cycle + *step* refresh a normal param edit
        // gets, so the display tracks the sweep tightly.
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*step*");
    }
    if step_print_tick.printed || step_print_tick.display_dirty {
        editor.mark_needs_redraw();
    }

    // 2. Sync reactive state AFTER events
    let ct = current_track_for_app(&mut app, &ctx.shared.current_track).unwrap_or(0);
    sync_watched_sampler_voices(
        &app,
        ct,
        &mut ctx.frame.watched_sampler_voice_track,
        &mut ctx.frame.watched_sampler_voice_ids,
    );
    let reactive_sync_started = Instant::now();
    {
        let playing = ctx.shared.state.transport.playing.load(Ordering::Relaxed);
        let bpm = ctx.shared.state.transport.bpm.load(Ordering::Relaxed);
        if ctx.meters.last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
            ctx.meters.cached_cpu_load_bits = ctx.shared.state.transport.cpu_load_pct.load(Ordering::Relaxed);
            ctx.meters.last_cpu_ui_poll_at = Instant::now();
        }
        let cpu_load_bits = ctx.meters.cached_cpu_load_bits;
        let output_latency_seconds = ctx.shared.state.pdc_latency_seconds();
        let output_latency_bits = output_latency_seconds.to_bits();
        let transport_playhead = ctx.shared.state.transport.playhead.load(Ordering::Relaxed);
        let playhead = ctx.shared.state.transport.track_playheads[ct].load(Ordering::Relaxed);
        let epoch = ctx.shared.state.transport.pattern_epoch.load(Ordering::Relaxed);
        let metal_visible = editor_has_visible_buffer(&editor, "*metal*");
        let mixer_visible = editor_has_visible_mixer_buffer(&editor);
        let sequencer_visible = editor_has_visible_buffer(&editor, "*sequencer*");
        let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
        let step_visible = editor_has_visible_buffer(&editor, "*step*");
        let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
        let master_meter_visible = transport_visible || mixer_visible;
        let track_and_bus_meter_visible =
            track_and_bus_meter_bindings_visible(mixer_visible, sequencer_visible);
        let current_track_playhead_visible = editor_has_visible_buffer(&editor, "*metal*")
            || editor_has_visible_buffer(&editor, "*piano-roll*");
        let previous_playhead = ctx.frame.prev_playhead;
        let current_track_playhead_changed = playhead != ctx.frame.prev_playhead;
        let mut meter_polled = false;
        if ctx.meters.last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL {
            ctx.meters.cached_peak_l_level = meter_display_level(f32::from_bits(
                ctx.shared.state.transport.peak_l.load(Ordering::Relaxed),
            ));
            ctx.meters.cached_peak_r_level = meter_display_level(f32::from_bits(
                ctx.shared.state.transport.peak_r.load(Ordering::Relaxed),
            ));
            ctx.meters.cached_track_peak_levels =
                read_track_peak_levels(app.graph.lg, &app.graph.track_node_ids);
            ctx.meters.cached_rack_slot_peak_levels =
                read_rack_slot_peak_levels(app.graph.lg, &app);
            ctx.meters.cached_bus_peak_levels =
                read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
            (
                ctx.meters.cached_modulator_phases,
                ctx.meters.cached_modulator_levels,
            ) = read_modulator_display_values(app.graph.lg, &app);
            meter_polled = true;
            ctx.meters.last_meter_poll_at = Instant::now();
        }
        // Effective (post-modulation) effect param values (eseq-dtx.13,
        // generalized in eseq-hpc). Gated on the FX panel: hidden panels drop
        // their modulator nodes off the watchlist and report base values, which
        // is also what settles knob dots and curves back to base when
        // modulation stops.
        //
        // Also polled off-cadence whenever fx_epoch moves, so a freshly
        // inserted effect publishes its base values in the same tick its panel
        // is built. Without that seed `bind-seq` vivifies 0.0 and e.g. a Filter
        // Table renders frame=0 / cutoff=0 Hz (off the 40..18000 log axis) for
        // up to a meter interval.
        //
        // The same pass samples the selected track's instrument (eseq-6mva),
        // whose per-voice modulators are read through the audio thread's
        // published last-triggered voice; the track is part of the poll gate so
        // switching instruments republishes immediately rather than leaving the
        // previous one's modulation on the panel.
        let mod_display_epoch = ctx.shared.fx_epoch.load(Ordering::Relaxed);
        if meter_polled
            || mod_display_epoch != ctx.meters.mod_display_poll_fx_epoch
            || Some(ct) != ctx.meters.mod_display_poll_track
        {
            ctx.meters.mod_display_poll_fx_epoch = mod_display_epoch;
            ctx.meters.mod_display_poll_track = Some(ct);
            let mod_display_selected_step = selected_plock_step(&ctx.shared.selected_steps);
            ctx.meters.cached_mod_display_values = read_mod_display_values(
                app.graph.lg,
                &app,
                &ctx.shared.state,
                Some(ct),
                mod_display_selected_step,
                fx_visible,
                &mut ctx.meters.watched_display_modulators,
            );
        }
        let mut needs_reactive_cycle = false;
        let mut refresh_visible_step_after_cycle = false;
        let selected_neural_snapshot = ctx.shared.selected_neural_neurons.lock().unwrap().clone();
        let track_active_notes: Vec<Vec<sequencer::sequencer::ActiveNoteActivity>> =
            (0..app.tracks.len())
            .map(|track| ctx.shared.state.active_note_activity(track))
            .collect();
        if track_active_notes != ctx.frame.prev_track_active_notes {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "track-active-notes",
                    build_track_active_notes_snapshot_value(&track_active_notes),
                )
                .effects_dirty;
            ctx.frame.prev_track_active_notes = track_active_notes.clone();
        }
        if fx_visible {
            let active_notes: Vec<u8> = track_active_notes
                .get(ct)
                .into_iter()
                .flatten()
                .map(|activity| activity.note)
                .collect();
            if active_notes != ctx.frame.prev_instrument_active_notes {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "instrument-active-notes",
                        build_active_notes_value(&active_notes),
                    )
                    .effects_dirty;
                ctx.frame.prev_instrument_active_notes = active_notes;
            }
        }
        if selected_neural_snapshot != ctx.frame.prev_selected_neural_neurons {
            needs_reactive_cycle |= sync_selected_neural_neuron_bindings(
                editor.runtime_mut(),
                &ctx.shared.state,
                &selected_neural_snapshot,
            );
            let revision =
                capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot);
            needs_reactive_cycle |= sync_fx_param_bindings_delta(
                &mut ctx.frame.fx_param_sync_revision,
                revision,
                editor.runtime_mut(),
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
            needs_reactive_cycle |= sync_track_plocks_for_neural_selection(
                editor.runtime_mut(),
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
            ctx.frame.prev_selected_neural_neurons = selected_neural_snapshot.clone();
        }
        // Track switch — rebuild everything
        if ct != ctx.frame.prev_current_track && !app.tracks.is_empty() {
            editor.reset_widget_scroll_for_buffer_named("*metal*");
            editor.reset_widget_scroll_for_buffer_named("*fx*");
            ctx.gesture.preview_plock_variant = None;
            let cleared_step_selection = {
                let mut selection = ctx.shared.selected_steps.lock().unwrap();
                let had_selection = !selection.is_empty();
                selection.clear();
                had_selection
            };
            let cleared_piano_selection = {
                let mut selection = ctx.shared.piano_roll_selection.lock().unwrap();
                let had_selection = !selection.is_empty();
                selection.clear();
                had_selection
            };
            if cleared_step_selection || cleared_piano_selection {
                ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
            }
            let _ = editor.runtime_mut().eval_str("(set! eseq.seq-core-state/selected-bus -1)");
            reset_sampler_waveform_view(&mut editor);
            let param_sync_revision =
                capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot);
            let rt = editor.runtime_mut();
            set_current_track_reactive(rt, app.tracks.len(), ct);
            if current_track_playhead_visible {
                sync_playhead_fields(
                    rt,
                    playhead as usize,
                    ctx.shared.state.pattern.track_params[ct].get_num_steps(),
                );
            }
            if transport_visible {
                rt.set_reactive(
                    "SEQ",
                    "transport-playhead",
                    Value::Number(transport_playhead as f64),
                );
            }
            rt.set_reactive("SEQ", "steps", build_steps_value(&ctx.shared.state, ct));
            sync_piano_roll_state(
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.piano_roll_selection,
            );
            sync_step_param_lists(rt, &ctx.shared.state, ct);
            // A track switch changes only track-addressed publication. Global
            // topology, mixer, meter, modulator, and accumulator snapshots are
            // unchanged and retain their already-published values.
            sync_track_params_delta(
                &mut ctx.frame.track_param_sync_revision,
                param_sync_revision.clone(),
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
            sync_fx_param_bindings_delta(
                &mut ctx.frame.fx_param_sync_revision,
                param_sync_revision,
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
            // The selecting native already issued the structural fx
            // revision for this owner change. Leave the panel values to
            // that branch below instead of building them twice. Direct
            // current-track writers that did not issue an fx revision
            // still get a complete destination panel here.
            let fx_ep = ctx.shared.fx_epoch.load(Ordering::Relaxed);
            let fx_value_ep = ctx.shared.fx_value_epoch.load(Ordering::Relaxed);
            if fx_visible
                && fx_ep == ctx.frame.prev_fx_epoch
                && fx_value_ep == ctx.frame.prev_fx_value_epoch
            {
                sync_fx_panel_state(
                    rt,
                    &app,
                    &ctx.shared.state,
                    Some(ct),
                    &ctx.shared.selected_steps,
                    true,
                );
            } else {
                if !fx_visible {
                    // Both this frame's publication and the fx-epoch catch-up
                    // branch below are gated on *fx* visibility, so nothing
                    // else refreshes the panel state that non-*fx* buffers
                    // read (*samples* / *macro-mappings* read
                    // SEQ.instrument-panel, *metal* reads
                    // SEQ.step-has-plocks). Publish that slice for the new
                    // track now; the heavier *fx*-only lists still wait for
                    // *fx* to be shown.
                    sync_shared_panel_state(
                        rt,
                        &app,
                        &ctx.shared.state,
                        Some(ct),
                        &ctx.shared.selected_steps,
                        true,
                    );
                }
                if fx_ep == ctx.frame.prev_fx_epoch {
                    // No structural fx revision is pending, yet the panel was
                    // not rebuilt above — either *fx* is hidden, or only a
                    // value revision is pending (a value patch cannot carry
                    // the new owner's panel STRUCTURE). Bump fx_epoch so the
                    // fx-epoch branch rebuilds structurally: this frame if
                    // visible, else on the first frame *fx* is shown again.
                    ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
            sync_sidebar_browser(rt, &app, ct);
            ctx.frame.prev_current_track = ct;
            ctx.frame.prev_playhead = playhead;
            ctx.frame.prev_transport_playhead = transport_playhead;
            ctx.frame.prev_pattern_epoch = epoch;
            needs_reactive_cycle = true;
        }

        // Track-groups reconcile: pull native-mutated groups (collapse toggle,
        // group create) into app.groups and rebuild the SEQ.groups reactive.
        {
            let groups_snapshot = ctx.shared.track_groups.lock().unwrap().clone();
            if groups_snapshot != ctx.frame.prev_groups {
                app.groups = groups_snapshot.clone();
                let rt = editor.runtime_mut();
                sync_groups_bindings(rt, &app.groups);
                ctx.frame.prev_groups = groups_snapshot;
                needs_reactive_cycle = true;
            }
        }

        // Rack pad-arm reconcile: the arm native mutates the shared handle;
        // the grid header reads SEQ.armed-rack-id (-1 = no rack armed).
        {
            let armed_rack = *ctx.shared.armed_rack.lock().unwrap();
            if armed_rack != ctx.frame.prev_armed_rack {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "armed-rack-id",
                    Value::Number(armed_rack.map(|id| id as f64).unwrap_or(-1.0)),
                );
                ctx.frame.prev_armed_rack = armed_rack;
                needs_reactive_cycle = true;
            }
        }

        // Multi-select highlight reconcile. Runs after the track-switch block
        // so it overrides the single-select bindings written there.
        {
            let selected_snapshot = ctx.shared.selected_tracks.lock().unwrap().clone();
            if selected_snapshot != ctx.frame.prev_selected_tracks {
                let rt = editor.runtime_mut();
                sync_selected_tracks_bindings(rt, app.tracks.len(), ct, &selected_snapshot);
                ctx.frame.prev_selected_tracks = selected_snapshot;
                needs_reactive_cycle = true;
            }
        }

        if playing != ctx.frame.prev_playing {
            // A transport flip means the user is playing/recording now: drop
            // any widget focus left by an earlier click, so global shortcuts
            // (record, live keys) stop landing in that widget.
            editor.blur_all_widget_focus();
            let param_sync_revision =
                capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot);
            let rt = editor.runtime_mut();
            rt.set_reactive("SEQ", "playing", Value::Bool(playing));
            if sequencer_visible {
                if playing {
                    sync_all_track_playhead_fields(rt, &ctx.shared.state, &app);
                } else {
                    clear_all_track_playhead_fields(rt, &app);
                }
            }
            ctx.frame.prev_playing = playing;
            needs_reactive_cycle = true;
            if (fx_visible || step_visible) && !app.tracks.is_empty() {
                let rt = editor.runtime_mut();
                sync_track_params_delta(
                    &mut ctx.frame.track_param_sync_revision,
                    param_sync_revision.clone(),
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                    &selected_neural_snapshot,
                );
                if ctx.gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                    *track != ct || !ctx.shared.selected_steps.lock().unwrap().is_empty()
                }) {
                    ctx.gesture.preview_plock_variant = None;
                }
                let preview_dirty = sync_track_plock_variant_preview(
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                    ctx.gesture.preview_plock_variant.as_ref(),
                );
                needs_reactive_cycle |= preview_dirty;
                refresh_visible_step_after_cycle |= preview_dirty;
                if fx_visible {
                    needs_reactive_cycle |= sync_fx_param_bindings_delta(
                        &mut ctx.frame.fx_param_sync_revision,
                        param_sync_revision,
                        rt,
                        &app,
                        &ctx.shared.state,
                        ct,
                        &ctx.shared.selected_steps,
                        &selected_neural_snapshot,
                    );
                }
            }
        }
        if bpm != ctx.frame.prev_bpm {
            app.push_all_delay_bpm();
            editor
                .runtime_mut()
                .set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
            ctx.frame.prev_bpm = bpm;
            needs_reactive_cycle = true;
        }
        if output_latency_bits != ctx.frame.prev_output_latency_bits {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "output-latency-ms",
                    Value::Number((output_latency_seconds * 1000.0) as f64),
                )
                .effects_dirty;
            ctx.frame.prev_output_latency_bits = output_latency_bits;
        }
        if transport_visible && cpu_load_bits != ctx.frame.prev_cpu_load_bits {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "cpu-load-pct",
                    Value::Number(f32::from_bits(cpu_load_bits) as f64),
                )
                .effects_dirty;
            ctx.frame.prev_cpu_load_bits = cpu_load_bits;
        }
        if !transport_visible && cpu_load_bits != ctx.frame.prev_cpu_load_bits {
            ctx.frame.prev_cpu_load_bits = cpu_load_bits;
        }
        let master_rec_on = ctx.shared.master_recording.load(Ordering::Acquire);
        app.ui.master_recording = master_rec_on;
        if transport_visible && master_rec_on != ctx.frame.prev_master_recording {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on))
                .effects_dirty;
            ctx.frame.prev_master_recording = master_rec_on;
        }
        if !transport_visible && master_rec_on != ctx.frame.prev_master_recording {
            ctx.frame.prev_master_recording = master_rec_on;
        }
        // Roll mode + rate (docs/rolling-core-spec.md 8): the atomics are the
        // source of truth (rate keys write them without touching the runtime),
        // so the transport display diffs them per frame like master-recording.
        let roll_mode_on = ctx
            .shared
            .state
            .transport
            .roll_mode
            .load(Ordering::Relaxed);
        if roll_mode_on != ctx.frame.prev_roll_mode {
            if transport_visible {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "roll-mode", Value::Bool(roll_mode_on))
                    .effects_dirty;
            }
            ctx.frame.prev_roll_mode = roll_mode_on;
        }
        let roll_rate_raw = ctx
            .shared
            .state
            .transport
            .roll_rate
            .load(Ordering::Relaxed);
        if roll_rate_raw != ctx.frame.prev_roll_rate {
            if transport_visible {
                let label = sequencer::sequencer::Timebase::from_index(roll_rate_raw).label();
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "roll-rate", Value::String(label.to_string()))
                    .effects_dirty;
            }
            ctx.frame.prev_roll_rate = roll_rate_raw;
        }
        let sequence_rolling = ctx
            .shared
            .state
            .transport
            .sequence_rolling
            .load(Ordering::Relaxed);
        if sequence_rolling != ctx.frame.prev_sequence_rolling {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive("SEQ", "sequence-rolling", Value::Bool(sequence_rolling))
                .effects_dirty;
            ctx.frame.prev_sequence_rolling = sequence_rolling;
        }
        let roll_window_bits: Vec<(u64, u64)> = (0..app.tracks.len().min(
            ctx.shared.state.transport.roll_window_starts.len(),
        ))
            .map(|track| {
                (
                    ctx.shared.state.transport.roll_window_starts[track]
                        .load(Ordering::Relaxed),
                    ctx.shared.state.transport.roll_window_lengths[track]
                        .load(Ordering::Relaxed),
                )
            })
            .collect();
        if roll_window_bits != ctx.frame.prev_roll_windows {
            let roll_windows = roll_window_bits
                .iter()
                .map(|(start_bits, length_bits)| {
                    let start = f64::from_bits(*start_bits);
                    let value = if start.is_nan() {
                        Value::Bool(false)
                    } else {
                        Value::List(vec![
                            Rc::new(RefCell::new(Value::Number(start))),
                            Rc::new(RefCell::new(Value::Number(f64::from_bits(*length_bits)))),
                        ])
                    };
                    Rc::new(RefCell::new(value))
                })
                .collect();
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive("SEQ", "roll-window", Value::List(roll_windows))
                .effects_dirty;
            ctx.frame.prev_roll_windows = roll_window_bits;
        }
        // Song-mode bindings (docs/song-mode-spec.md 12): diff-published each
        // frame; the arrangement is re-read only on committed-song revision
        // change, and the lane surfaces derived from it diff by value.
        // The render-rate song position drives the transport readout and the
        // arrangement playhead, so it publishes while either is visible.
        let arrangement_visible = editor_has_visible_buffer(&editor, "*arrangement*");
        // Clip selection is dormant while the timeline is off screen (takes
        // spec 16.6), so the binding needs the view state before it resolves.
        app.set_arrangement_view_visible(arrangement_visible);
        // Sound binding (takes spec 16.2): keep the live device mirror on the
        // bound source before anything reads it. This is where a song row
        // transition (rule 2) re-binds the panel and the monitor sound, and
        // where a lane released by a session save-back is reloaded.
        app.sync_track_sound_bindings();
        // A binding move rewrites the mirror's devices without touching the
        // pattern epoch, so the panels would keep showing the old source's
        // knobs (and the old badge) until some unrelated edit republished
        // them. Drive the same rebuild a device change does.
        if app.sound_binding_epoch != ctx.frame.prev_sound_binding_epoch {
            ctx.frame.prev_sound_binding_epoch = app.sound_binding_epoch;
            ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
            // The FX panels carry their values in the `effects` map, but every
            // instrument knob reads a per-param `SEQ` value field that only a
            // param sync republishes — without this the instrument panel keeps
            // the previous source's knob positions while the FX panel updates.
            let revision =
                capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot);
            needs_reactive_cycle |= sync_fx_param_bindings_delta(
                &mut ctx.frame.fx_param_sync_revision,
                revision,
                editor.runtime_mut(),
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
        }
        needs_reactive_cycle |= sync_song_state(
            editor.runtime_mut(),
            &app,
            &mut ctx.frame.song,
            transport_visible || arrangement_visible,
        );
        needs_reactive_cycle |= sync_sound_palette(
            editor.runtime_mut(),
            &app,
            &mut ctx.frame.sound_palette,
            arrangement_visible,
        );
        if master_meter_visible && ctx.meters.cached_peak_l_level != ctx.frame.prev_peak_l_level {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "master-peak-l",
                    Value::Number(ctx.meters.cached_peak_l_level),
                )
                .effects_dirty;
            ctx.frame.prev_peak_l_level = ctx.meters.cached_peak_l_level;
        }
        if !master_meter_visible && ctx.meters.cached_peak_l_level != ctx.frame.prev_peak_l_level {
            ctx.frame.prev_peak_l_level = ctx.meters.cached_peak_l_level;
        }
        if master_meter_visible && ctx.meters.cached_peak_r_level != ctx.frame.prev_peak_r_level {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "master-peak-r",
                    Value::Number(ctx.meters.cached_peak_r_level),
                )
                .effects_dirty;
            ctx.frame.prev_peak_r_level = ctx.meters.cached_peak_r_level;
        }
        if !master_meter_visible && ctx.meters.cached_peak_r_level != ctx.frame.prev_peak_r_level {
            ctx.frame.prev_peak_r_level = ctx.meters.cached_peak_r_level;
        }
        if ctx.meters.cached_track_peak_levels != ctx.frame.prev_track_peak_levels {
            if track_and_bus_meter_visible {
                needs_reactive_cycle |= sync_track_peak_field_delta(
                    editor.runtime_mut(),
                    &ctx.frame.prev_track_peak_levels,
                    &ctx.meters.cached_track_peak_levels,
                );
            }
            ctx.frame.prev_track_peak_levels = ctx.meters.cached_track_peak_levels.clone();
        }
        if ctx.meters.cached_rack_slot_peak_levels != ctx.frame.prev_rack_slot_peak_levels {
            if track_and_bus_meter_visible {
                needs_reactive_cycle |= sync_rack_slot_peak_field_delta(
                    editor.runtime_mut(),
                    &ctx.frame.prev_rack_slot_peak_levels,
                    &ctx.meters.cached_rack_slot_peak_levels,
                );
            }
            ctx.frame.prev_rack_slot_peak_levels = ctx.meters.cached_rack_slot_peak_levels.clone();
        }
        if ctx.meters.cached_bus_peak_levels != ctx.frame.prev_bus_peak_levels {
            if track_and_bus_meter_visible {
                needs_reactive_cycle |= sync_bus_peak_field_delta(
                    editor.runtime_mut(),
                    &ctx.frame.prev_bus_peak_levels,
                    &ctx.meters.cached_bus_peak_levels,
                );
            }
            ctx.frame.prev_bus_peak_levels = ctx.meters.cached_bus_peak_levels.clone();
        }
        if ctx.meters.last_neural_visualization_poll_at.elapsed() >= NEURAL_VISUALIZATION_POLL_INTERVAL {
            ctx.meters.last_neural_visualization_poll_at = Instant::now();
            needs_reactive_cycle |= sync_neural_visualization_fields(
                editor.runtime_mut(),
                &ctx.shared.state,
                &mut ctx.meters.visualization_liveness,
            );
        }
        // Drum-rack pad lights (eseq-4b5.16). The flags are read every tick —
        // reading is what consumes the audio thread's trigger latch, so it must
        // not be skipped — but publishing is gated on the panel that draws
        // them, exactly as the modulator readouts below are.
        {
            let rack_pad_triggers = read_rack_pad_trigger_flags(
                &app,
                &ctx.shared.state,
                &track_active_notes,
                &mut ctx.frame.rack_pad_triggered_at,
                Instant::now(),
            );
            // `prev` mirrors what the runtime holds, so it is NOT updated
            // while the panel is hidden: the first visible tick then publishes
            // exactly what changed in the meantime — a pad that decayed while
            // hidden goes dark, one that lit comes up — instead of leaving a
            // stale light behind.
            if fx_visible && rack_pad_triggers != ctx.frame.prev_rack_pad_triggers {
                needs_reactive_cycle |= sync_rack_pad_trigger_field_delta(
                    editor.runtime_mut(),
                    &ctx.frame.prev_rack_pad_triggers,
                    &rack_pad_triggers,
                );
                ctx.frame.prev_rack_pad_triggers = rack_pad_triggers;
            }
        }
        if ctx.meters.cached_modulator_phases != ctx.frame.prev_modulator_phases {
            if fx_visible {
                needs_reactive_cycle |= sync_modulator_phase_field_delta(
                    editor.runtime_mut(),
                    &ctx.frame.prev_modulator_phases,
                    &ctx.meters.cached_modulator_phases,
                );
            }
            ctx.frame.prev_modulator_phases = ctx.meters.cached_modulator_phases.clone();
        }
        if ctx.meters.cached_modulator_levels != ctx.frame.prev_modulator_levels {
            if fx_visible {
                needs_reactive_cycle |= sync_modulator_level_field_delta(
                    editor.runtime_mut(),
                    &ctx.frame.prev_modulator_levels,
                    &ctx.meters.cached_modulator_levels,
                );
            }
            ctx.frame.prev_modulator_levels = ctx.meters.cached_modulator_levels.clone();
        }
        // Effective-value bindings (eseq-dtx.13, eseq-hpc). Published whatever
        // the panel visibility: the sampler already reports base values while
        // the FX panel is hidden, so this is what leaves the fields holding
        // base values for the next open, and it only writes on change.
        if ctx.meters.cached_mod_display_values != ctx.frame.prev_mod_display_values {
            needs_reactive_cycle |= sync_effect_mod_offset_field_delta(
                editor.runtime_mut(),
                &ctx.frame.prev_mod_display_values.effects,
                &ctx.meters.cached_mod_display_values.effects,
            )
            .0;
            needs_reactive_cycle |= sync_instrument_mod_offset_field_delta(
                editor.runtime_mut(),
                ctx.frame.prev_mod_display_values.instrument.as_ref(),
                ctx.meters.cached_mod_display_values.instrument.as_ref(),
            )
            .0;
            ctx.frame.prev_mod_display_values = ctx.meters.cached_mod_display_values.clone();
        }
        if sequencer_visible {
            let previous_track_playheads = ctx.frame.prev_track_playheads.clone();
            if sync_track_playhead_field_delta(
                editor.runtime_mut(),
                &ctx.shared.state,
                &app,
                &mut ctx.frame.prev_track_playheads,
            ) {
                needs_reactive_cycle = true;
            }
            if previous_track_playheads != ctx.frame.prev_track_playheads {
                let auto_follow_now = auto_follow_enabled(&ctx.shared.auto_follow_override_until);
                let selection_empty = ctx.shared.selected_steps.lock().unwrap().is_empty();
                let selected = ctx.shared.selected_steps.lock().unwrap();
                let rt = editor.runtime_mut();
                for mut viewport in ctx.shared.expanded_step_projection.all_viewports() {
                    if viewport.track >= app.tracks.len() {
                        continue;
                    }
                    let active_step = track_active_playhead_step(&ctx.shared.state, viewport.track);
                    let active_page = active_step / PAGE_SIZE;
                    if playing && auto_follow_now && selection_empty {
                        if viewport.page != active_page {
                            viewport.page = active_page;
                            viewport.cursor_step = active_step;
                            ctx.shared.expanded_step_projection.set_viewport(viewport);
                            needs_reactive_cycle |= sync_expanded_step_viewport(
                                rt, &ctx.shared.state, &app, &selected, ct, viewport,
                            );
                            continue;
                        }
                    }
                    needs_reactive_cycle |=
                        sync_expanded_step_viewport_playhead(rt, &ctx.shared.state, viewport);
                }
            }
        } else {
            ctx.frame.prev_track_playheads = track_playheads_snapshot(&ctx.shared.state, &app);
        }
        if current_track_playhead_visible
            && (!ctx.frame.prev_current_track_playhead_visible || playhead != ctx.frame.prev_playhead)
            && !app.tracks.is_empty()
        {
            if ctx.frame.prev_current_track_playhead_visible {
                needs_reactive_cycle |= sync_playhead_field_delta(
                    editor.runtime_mut(),
                    ctx.frame.prev_playhead as usize,
                    playhead as usize,
                    ctx.shared.state.pattern.track_params[ct].get_num_steps(),
                );
            } else {
                needs_reactive_cycle |= sync_playhead_fields(
                    editor.runtime_mut(),
                    playhead as usize,
                    ctx.shared.state.pattern.track_params[ct].get_num_steps(),
                );
            }
            ctx.frame.prev_playhead = playhead;
        }
        if !current_track_playhead_visible && ctx.frame.prev_playhead != playhead {
            ctx.frame.prev_playhead = playhead;
        }
        if (fx_visible || step_visible || mixer_visible)
            && current_track_playhead_changed
            && !app.tracks.is_empty()
        {
            let last_step = ctx.shared.state.pattern.track_params[ct]
                .get_num_steps()
                .max(1)
                .min(sequencer::sequencer::MAX_STEPS)
                .saturating_sub(1);
            let previous_step = (previous_playhead as usize).min(last_step);
            let current_step = (playhead as usize).min(last_step);
            let displayed_param_value_may_change = playhead_transition_changes_param_bindings(
                &ctx.shared.state,
                ct,
                &app.graph.effect_descriptors,
                &ctx.shared.selected_steps,
                previous_step,
                current_step,
            );
            let param_sync_revision = displayed_param_value_may_change
                .then(|| capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot));
            let rt = editor.runtime_mut();
            if displayed_param_value_may_change {
                needs_reactive_cycle |= sync_track_selection_param_binding_fields(
                    rt,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                );
                needs_reactive_cycle |= sync_selected_track_bus_send_binding_fields(
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                );
            }
            if ctx.gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                *track != ct || !ctx.shared.selected_steps.lock().unwrap().is_empty()
            }) {
                ctx.gesture.preview_plock_variant = None;
            }
            let preview_dirty = sync_track_plock_variant_preview(
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                ctx.gesture.preview_plock_variant.as_ref(),
            );
            refresh_visible_step_after_cycle |= preview_dirty;
            if fx_visible {
                if let Some(param_sync_revision) = param_sync_revision {
                    needs_reactive_cycle |= sync_fx_param_bindings_delta(
                        &mut ctx.frame.fx_param_sync_revision,
                        param_sync_revision,
                        rt,
                        &app,
                        &ctx.shared.state,
                        ct,
                        &ctx.shared.selected_steps,
                        &selected_neural_snapshot,
                    );
                }
            }
            needs_reactive_cycle |= preview_dirty;
        }
        ctx.frame.prev_current_track_playhead_visible = current_track_playhead_visible;
        let mut profile_pattern_reactive_cycle = false;
        let mut refresh_visible_sequencer_after_cycle = false;
        let mut refresh_visible_mixer_after_cycle = false;
        let mut refresh_visible_samples_after_cycle = false;
        let typed_invalidations = ctx.shared.ui_invalidations.drain();
        if apply_ui_invalidations(
            typed_invalidations,
            UiInvalidationApplyCtx {
                app: &mut app,
                editor: &mut editor,
                state: &ctx.shared.state,
                track_collapsed: &ctx.shared.track_collapsed,
                bus_state: &ctx.shared.bus_state,
                current_track_idx: ct,
                selected_steps: &ctx.shared.selected_steps,
                selected_neural_neurons: &selected_neural_snapshot,
                piano_roll_selection: &ctx.shared.piano_roll_selection,
                accumulator_names: &ctx.shared.accumulator_names,
                cached_track_peak_levels: &ctx.meters.cached_track_peak_levels,
                cached_bus_peak_levels: &ctx.meters.cached_bus_peak_levels,
                record_armed: &ctx.shared.record_armed,
                active_delete_target: &ctx.shared.active_delete_target,
                active_delete_target_version: &ctx.shared.active_delete_target_version,
                expanded_step_projection: &ctx.shared.expanded_step_projection,
                fx_visible,
                sequencer_visible,
                mixer_visible,
            },
        ) {
            needs_reactive_cycle = true;
        }
        // Edit-focus refresh (clip-edit-target spec 3): project the
        // App-resolved target into the cell the `seq-piano-roll-action`
        // native reads, and re-sync the piano roll whenever the focus itself
        // moved — a clip bind/unbind, a scene launch changing the effective
        // pattern under a pinned id, or a source dying (spec 3.3.1).
        {
            let focus = PianoRollFocusSpec::from_focus(app.track_edit_focus(ct));
            let focus_changed = {
                let mut cell = ctx.shared.piano_roll_focus.lock().unwrap();
                std::mem::replace(&mut *cell, focus) != focus
            };
            // The clip-shaped surfaces (`focus-clip-*`, the window overlay,
            // the clip-use label) are keyed off the clip SELECTION, not the
            // resolved write focus: two clips over the same pool pattern both
            // resolve `Pool(p)`, and a pinned clip whose pattern is the
            // effective one resolves `Live`. Diff the selection identity —
            // plus the committed-song revision, which moves whenever the
            // clip's start/end/offset does — so a re-select never leaves the
            // panel and the overlay on the previous clip's numbers.
            let clip_surface_key = (
                app.song_clip_selection
                    .map(|selection| (selection.track, selection.clip_id.0)),
                app.focus_clip_source_kind(ct),
                ctx.shared.state.committed_song_revision(),
            );
            let clip_surface_changed = ctx.frame.prev_focus_clip_surface != clip_surface_key;
            ctx.frame.prev_focus_clip_surface = clip_surface_key;
            if focus_changed {
                // The note set under the editor was just replaced, so any
                // surviving selection would address the *new* source's ids
                // (delete/nudge/move all act on the raw id set). Drop it the
                // same way a track switch does.
                let cleared_piano_selection = {
                    let mut selection = ctx.shared.piano_roll_selection.lock().unwrap();
                    let had_selection = !selection.is_empty();
                    selection.clear();
                    had_selection
                };
                *ctx.shared.piano_roll_move_state.lock().unwrap() = None;
                if cleared_piano_selection {
                    ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
            if focus_changed || clip_surface_changed {
                let rt = editor.runtime_mut();
                sync_piano_roll_state(
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.piano_roll_selection,
                );
                needs_reactive_cycle = true;
            }
            if current_track_playhead_visible {
                needs_reactive_cycle |=
                    sync_piano_roll_playhead(editor.runtime_mut(), &app, ct, playhead as usize);
            }
        }
        let mirror_epoch = app.song_row_mirror_epoch;
        if (epoch != ctx.frame.prev_pattern_epoch
            || mirror_epoch != ctx.frame.prev_song_row_mirror_epoch)
            && !app.tracks.is_empty()
        {
            let profile_switch = pattern_switch_profile_enabled();
            let profile_total_started = Instant::now();
            let sync_names_pattern_elapsed;
            let mut sync_playhead_elapsed = Duration::ZERO;
            let sync_current_steps_elapsed;
            let sync_sequencer_elapsed;
            let sync_expanded_elapsed;
            let sync_piano_elapsed;
            let sync_step_params_elapsed;
            let sync_mixer_elapsed;
            let sync_track_params_elapsed;
            let sync_fx_bindings_elapsed;
            let sync_plocks_sidebar_elapsed;
            let old_pattern_epoch = ctx.frame.prev_pattern_epoch;
            let selected_neural_snapshot =
                ctx.shared.selected_neural_neurons.lock().unwrap().clone();
            let param_sync_revision =
                capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot);
            let rt = editor.runtime_mut();
            let started = Instant::now();
            sync_shared_track_collapsed(&ctx.shared.track_collapsed, &app);
            sync_track_name_state(rt, &mut *ctx.track_names, &app);
            sync_pattern_state(rt, &ctx.shared.state);
            sync_selected_neural_neuron_bindings(rt, &ctx.shared.state, &selected_neural_snapshot);
            sync_names_pattern_elapsed = started.elapsed();
            if current_track_playhead_visible {
                let started = Instant::now();
                sync_playhead_fields(
                    rt,
                    playhead as usize,
                    ctx.shared.state.pattern.track_params[ct].get_num_steps(),
                );
                sync_playhead_elapsed = started.elapsed();
            }
            let started = Instant::now();
            rt.set_reactive("SEQ", "steps", build_steps_value(&ctx.shared.state, ct));
            sync_current_steps_elapsed = started.elapsed();
            let started = Instant::now();
            sync_all_track_sequencer_state(
                rt,
                &ctx.shared.state,
                &app,
                ct,
                &ctx.shared.selected_steps,
            );
            sync_sequencer_elapsed = started.elapsed();
            let started = Instant::now();
            if sequencer_visible {
                let _ = sync_all_expanded_step_viewports(
                    rt,
                    &ctx.shared.state,
                    &app,
                    &ctx.shared.selected_steps,
                    ct,
                    &ctx.shared.expanded_step_projection,
                );
            }
            sync_expanded_elapsed = started.elapsed();
            let started = Instant::now();
            sync_piano_roll_state(
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.piano_roll_selection,
            );
            sync_piano_elapsed = started.elapsed();
            let started = Instant::now();
            sync_step_param_lists(rt, &ctx.shared.state, ct);
            sync_step_params_elapsed = started.elapsed();
            let started = Instant::now();
            sync_track_mixer_state(rt, &app, &ctx.shared.state);
            sync_bus_mixer_state(rt, &app);
            if track_and_bus_meter_visible {
                sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
                sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
            }
            sync_mixer_elapsed = started.elapsed();
            *ctx.shared.accumulator_names.lock().unwrap() = build_accumulator_names(&app);
            let started = Instant::now();
            sync_track_params_delta(
                &mut ctx.frame.track_param_sync_revision,
                param_sync_revision.clone(),
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
            if ctx.gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                *track != ct || !ctx.shared.selected_steps.lock().unwrap().is_empty()
            }) {
                ctx.gesture.preview_plock_variant = None;
            }
            let preview_dirty = sync_track_plock_variant_preview(
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                ctx.gesture.preview_plock_variant.as_ref(),
            );
            refresh_visible_step_after_cycle |= preview_dirty;
            sync_track_params_elapsed = started.elapsed();
            let started = Instant::now();
            sync_fx_param_bindings_delta(
                &mut ctx.frame.fx_param_sync_revision,
                param_sync_revision,
                rt,
                &app,
                &ctx.shared.state,
                ct,
                &ctx.shared.selected_steps,
                &selected_neural_snapshot,
            );
            sync_fx_bindings_elapsed = started.elapsed();
            ctx.frame.prev_selected_neural_neurons = selected_neural_snapshot;
            let started = Instant::now();
            rt.set_reactive(
                "SEQ",
                "step-has-plocks",
                build_step_has_plocks(&ctx.shared.state, ct, &app.graph.effect_descriptors),
            );
            sync_sidebar_browser(rt, &app, ct);
            sync_plocks_sidebar_elapsed = started.elapsed();
            if profile_switch {
                eprintln!(
                    "[pattern-switch-profile][epoch-sync] total={:.2}ms epoch {}->{} names_pattern={:.2}ms playhead={:.2}ms current_steps={:.2}ms sequencer_bindings={:.2}ms expanded_step_viewports={:.2}ms piano={:.2}ms step_params={:.2}ms mixer={:.2}ms track_params={:.2}ms fx_bindings={:.2}ms plocks_sidebar={:.2}ms",
                    duration_ms(profile_total_started.elapsed()),
                    old_pattern_epoch,
                    epoch,
                    duration_ms(sync_names_pattern_elapsed),
                    duration_ms(sync_playhead_elapsed),
                    duration_ms(sync_current_steps_elapsed),
                    duration_ms(sync_sequencer_elapsed),
                    duration_ms(sync_expanded_elapsed),
                    duration_ms(sync_piano_elapsed),
                    duration_ms(sync_step_params_elapsed),
                    duration_ms(sync_mixer_elapsed),
                    duration_ms(sync_track_params_elapsed),
                    duration_ms(sync_fx_bindings_elapsed),
                    duration_ms(sync_plocks_sidebar_elapsed),
                );
            }
            ctx.frame.prev_pattern_epoch = epoch;
            ctx.frame.prev_song_row_mirror_epoch = mirror_epoch;
            ctx.frame.prev_track_button_states = track_button_state_snapshot(&ctx.shared.state);
            needs_reactive_cycle = true;
            refresh_visible_mixer_after_cycle |= mixer_visible;
            profile_pattern_reactive_cycle = profile_switch;
        }
        // Delete-target arm/clear rides its own version counter instead of
        // ui_epoch: the gesture only moves the delete-target read surfaces,
        // and a full project resync per clip-launch click (which arms the
        // launched cell as the delete target) costs ~7ms at 20-clip pools.
        let delete_target_version = ctx
            .shared
            .active_delete_target_version
            .load(Ordering::Relaxed);
        if delete_target_version != ctx.frame.prev_delete_target_version {
            ctx.frame.prev_delete_target_version = delete_target_version;
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(delete_target_version as f64),
            );
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &ctx.shared.state,
                ctx.shared.active_delete_target.lock().unwrap().as_ref(),
            );
            needs_reactive_cycle = true;
        }
        let ui_ep = ctx.shared.ui_epoch.load(Ordering::Relaxed);
        if ui_ep != ctx.frame.prev_ui_epoch {
            if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                eprintln!(
                    "[ui-trace][metal_seq] ui_epoch {}->{} visible metal={} mixer={} sequencer={} fx={} ct={}",
                    ctx.frame.prev_ui_epoch,
                    ui_ep,
                    metal_visible,
                    mixer_visible,
                    sequencer_visible,
                    fx_visible,
                    ct
                );
            }
            pull_shared_bus_state(&mut app, &ctx.shared.bus_state);
            let track_button_states = track_button_state_snapshot(&ctx.shared.state);
            let track_buttons_changed = track_button_states != ctx.frame.prev_track_button_states;
            if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                eprintln!(
                    "[ui-trace][metal_seq] track_buttons_changed={} prev_buttons={} next_buttons={}",
                    track_buttons_changed,
                    ctx.frame.prev_track_button_states.len(),
                    track_button_states.len()
                );
            }
            let param_sync_revision = (!app.tracks.is_empty())
                .then(|| capture_param_sync_revision(&app, ctx, ct, &selected_neural_snapshot));
            let rt = editor.runtime_mut();
            sync_macro_state(rt, &app);
            if app.tracks.is_empty() {
                sync_track_topology_state(
                    rt,
                    &app,
                    &ctx.shared.state,
                    &mut *ctx.track_names,
                    ct,
                    &ctx.shared.selected_steps,
                    &ctx.shared.piano_roll_selection,
                    &ctx.shared.accumulator_names,
                    &ctx.shared.record_armed,
                    &ctx.meters.cached_track_peak_levels,
                );
                sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
            } else {
                let param_sync_revision =
                    param_sync_revision.expect("nonempty track state has a sync revision");
                sync_shared_track_collapsed(&ctx.shared.track_collapsed, &app);
                sync_track_name_state(rt, &mut *ctx.track_names, &app);
                rt.set_reactive("SEQ", "steps", build_steps_value(&ctx.shared.state, ct));
                sync_step_param_lists(rt, &ctx.shared.state, ct);
                if metal_visible || sequencer_visible {
                    sync_all_track_sequencer_state(
                        rt,
                        &ctx.shared.state,
                        &app,
                        ct,
                        &ctx.shared.selected_steps,
                    );
                }
                if sequencer_visible {
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        &ctx.shared.state,
                        &app,
                        &ctx.shared.selected_steps,
                        ct,
                        &ctx.shared.expanded_step_projection,
                    );
                }
                sync_track_mixer_state(rt, &app, &ctx.shared.state);
                sync_bus_mixer_state(rt, &app);
                sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
                sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
                *ctx.shared.accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params_delta(
                    &mut ctx.frame.track_param_sync_revision,
                    param_sync_revision.clone(),
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                    &selected_neural_snapshot,
                );
                if ctx.gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                    *track != ct || !ctx.shared.selected_steps.lock().unwrap().is_empty()
                }) {
                    ctx.gesture.preview_plock_variant = None;
                }
                let preview_dirty = sync_track_plock_variant_preview(
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                    ctx.gesture.preview_plock_variant.as_ref(),
                );
                refresh_visible_step_after_cycle |= preview_dirty;
                sync_fx_param_bindings_delta(
                    &mut ctx.frame.fx_param_sync_revision,
                    param_sync_revision,
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.selected_steps,
                    &selected_neural_snapshot,
                );
                rt.set_reactive(
                    "SEQ",
                    "selected-steps",
                    build_selection_value(&ctx.shared.selected_steps),
                );
                sync_piano_roll_state(
                    rt,
                    &app,
                    &ctx.shared.state,
                    ct,
                    &ctx.shared.piano_roll_selection,
                );
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&ctx.shared.state, ct, &app.graph.effect_descriptors),
                );
            }
            // Sync recording state
            let rec_on = ctx.shared.recording.load(Ordering::Relaxed);
            let master_rec_on = ctx.shared.master_recording.load(Ordering::Acquire);
            rt.set_reactive("SEQ", "recording", Value::Bool(rec_on));
            rt.set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on));
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(
                    ctx.shared
                        .active_delete_target_version
                        .load(Ordering::Relaxed) as f64,
                ),
            );
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &ctx.shared.state,
                ctx.shared.active_delete_target.lock().unwrap().as_ref(),
            );
            if app.record_arm_sync_pending {
                // Project load restored per-track arm flags (takes spec
                // 8.1): push them INTO the shared vector once — the per-tick
                // sync below runs the other way (shared -> app).
                app.record_arm_sync_pending = false;
                let mut armed = ctx.shared.record_armed.lock().unwrap();
                armed.clear();
                armed.extend(app.graph.record_armed.iter().copied());
            }
            let armed = ctx.shared.record_armed.lock().unwrap();
            let record_armed_changed = armed.len() != app.graph.record_armed.len()
                || armed
                    .iter()
                    .enumerate()
                    .any(|(i, armed)| app.graph.record_armed.get(i) != Some(armed));
            rt.set_reactive("SEQ", "record-armed", build_record_armed_value(&armed));
            // Sync to app for TUI recording logic
            app.ui.recording = rec_on;
            app.ui.master_recording = master_rec_on;
            ctx.frame.prev_master_recording = master_rec_on;
            for (i, a) in armed.iter().enumerate() {
                if i < app.graph.record_armed.len() {
                    app.graph.record_armed[i] = *a;
                }
            }
            refresh_visible_sequencer_after_cycle = sequencer_visible;
            refresh_visible_mixer_after_cycle |=
                mixer_visible && (record_armed_changed || track_buttons_changed);
            if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                eprintln!(
                    "[ui-trace][metal_seq] refresh_after_cycle sequencer={} mixer={} record_armed_changed={} track_buttons_changed={}",
                    refresh_visible_sequencer_after_cycle,
                    refresh_visible_mixer_after_cycle,
                    record_armed_changed,
                    track_buttons_changed
                );
            }
            ctx.frame.prev_track_button_states = track_button_states;
            ctx.frame.prev_ui_epoch = ui_ep;
            needs_reactive_cycle = true;
        }
        let fx_ep = ctx.shared.fx_epoch.load(Ordering::Relaxed);
        let fx_value_ep = ctx.shared.fx_value_epoch.load(Ordering::Relaxed);
        if fx_visible
            && (fx_ep != ctx.frame.prev_fx_epoch
                || fx_value_ep != ctx.frame.prev_fx_value_epoch)
        {
            // fx_epoch = STRUCTURAL invalidation (Boolean/Enum param edits,
            // add/remove effect): full set_reactive so the *fx* root re-evals
            // conditional layout. fx_value_epoch alone = value-only scene/clip
            // launch: in-place patch, field bindings carry the visuals.
            let structural = fx_ep != ctx.frame.prev_fx_epoch;
            sync_fx_panel_state(
                editor.runtime_mut(),
                &app,
                &ctx.shared.state,
                (!app.tracks.is_empty()).then_some(ct),
                &ctx.shared.selected_steps,
                structural,
            );
            ctx.frame.prev_fx_epoch = fx_ep;
            ctx.frame.prev_fx_value_epoch = fx_value_ep;
            needs_reactive_cycle = true;
        }
        if transport_visible && transport_playhead != ctx.frame.prev_transport_playhead {
            needs_reactive_cycle |= editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "transport-playhead",
                    Value::Number(transport_playhead as f64),
                )
                .effects_dirty;
            ctx.frame.prev_transport_playhead = transport_playhead;
        }
        if !transport_visible && transport_playhead != ctx.frame.prev_transport_playhead {
            ctx.frame.prev_transport_playhead = transport_playhead;
        }
        {
            let analysis_generation = app.sample_analysis.cache().generation();
            if analysis_generation != ctx.frame.prev_sampler_analysis_generation {
                app.publish_all_sampler_analysis_runtime();
                ctx.frame.prev_sampler_analysis_generation = analysis_generation;
            }
            let ct = ctx.shared.current_track.load(Ordering::Relaxed);
            let analysis_key = if app.is_sampler_track(ct) {
                let buffer_id = app.graph.track_buffer_ids.get(ct).copied().unwrap_or(-1);
                let entry = app.sample_analysis.cache().get(buffer_id);
                let (status, bpm_bits, onset_count) = match entry.as_deref() {
                    Some(sequencer::analysis::AnalysisEntry::Pending) => (1, 0, 0),
                    Some(sequencer::analysis::AnalysisEntry::Ready(result)) => {
                        (2, result.bpm.to_bits(), result.onsets_frames.len())
                    }
                    Some(sequencer::analysis::AnalysisEntry::Failed(_)) => (3, 0, 0),
                    None => (0, 0, 0),
                };
                Some((ct, buffer_id, status, bpm_bits, onset_count))
            } else {
                None
            };
            if analysis_key != ctx.frame.prev_sampler_analysis_key {
                if let Some((ct, _, _, _, _)) = analysis_key {
                    app.publish_sampler_analysis_runtime(ct);
                    editor.runtime_mut().set_reactive(
                        "SEQ",
                        "instrument-panel",
                        build_instrument_panel_value(&app, ct, &ctx.shared.selected_steps),
                    );
                    needs_reactive_cycle = true;
                }
                ctx.frame.prev_sampler_analysis_key = analysis_key;
            }
        }
        // Update sampler playhead for waveform display
        {
            let ct = ctx.shared.current_track.load(Ordering::Relaxed);
            if app.is_sampler_track(ct) {
                let ph = read_sampler_playhead_seconds(&app, ct);
                if ph > 0.0 {
                    editor
                        .runtime_mut()
                        .set_reactive("SEQ", "sampler-playhead", Value::Number(ph));
                    needs_reactive_cycle = true;
                }
            }
        }
        // Browser sample preview: mirror the audio-thread preview state so the
        // play button and the strip's playhead track real playback (including
        // the clip ending on its own).
        {
            let preview_playing = sequencer::audio::preview::is_playing();
            if preview_playing != ctx.frame.prev_browser_preview_playing {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "browser-preview-playing",
                    Value::Bool(preview_playing),
                );
                if !preview_playing {
                    editor.runtime_mut().set_reactive(
                        "SEQ",
                        "browser-preview-playhead",
                        Value::Number(0.0),
                    );
                }
                ctx.frame.prev_browser_preview_playing = preview_playing;
                needs_reactive_cycle = true;
            }
            if preview_playing {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "browser-preview-playhead",
                    Value::Number(sequencer::audio::preview::position_seconds()),
                );
                needs_reactive_cycle = true;
            }
        }
        let auto_follow = auto_follow_enabled(&ctx.shared.auto_follow_override_until);
        if auto_follow != ctx.frame.prev_auto_follow {
            editor
                .runtime_mut()
                .set_reactive("SEQ", "auto-follow", Value::Bool(auto_follow));
            ctx.frame.prev_auto_follow = auto_follow;
            needs_reactive_cycle = true;
        }
        // Macro-action buttons (save-to-library / fork). The query itself reads
        // the session source and the macro library, so it is gated on a
        // fingerprint of those inputs; diffing only the published strings left
        // the lookup running on every tick.
        let editor_macro_action_fingerprint = editor_macro_action_fingerprint(ctx.sessions);
        if editor_macro_action_fingerprint != ctx.frame.prev_editor_macro_action_fingerprint {
            let editor_macro_action = ctx.sessions.instrument_edit_session
                .as_ref()
                .and_then(active_instrument_editor_macro_action)
                .or_else(|| {
                    ctx.sessions.effect_edit_session
                        .as_ref()
                        .and_then(active_effect_editor_macro_action)
                });
            let editor_macro_action = editor_macro_action_strings(editor_macro_action.as_ref());
            ctx.frame.prev_editor_macro_action_fingerprint = editor_macro_action_fingerprint;
            if editor_macro_action != ctx.frame.prev_editor_macro_action {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-active-macro-name",
                    Value::String(editor_macro_action.0.clone()),
                );
                rt.set_reactive(
                    "SEQ",
                    "editor-active-macro-action",
                    Value::String(editor_macro_action.1.clone()),
                );
                ctx.frame.prev_editor_macro_action = editor_macro_action;
                refresh_visible_samples_after_cycle = true;
                needs_reactive_cycle = true;
            }
        }
        // Macro sidebar: local defmacros scanned from the active edit-session
        // source, plus the saved-macro library. Republished only when the
        // session source changes (the library is re-read then too — saving a
        // macro to the library rewrites the source, so the two coincide).
        let editor_patch_source = ctx
            .sessions
            .instrument_edit_session
            .as_ref()
            .map(|session| session.last_valid_source.as_str())
            .or_else(|| {
                ctx.sessions
                    .effect_edit_session
                    .as_ref()
                    .map(|session| session.last_valid_source.as_str())
            });
        let sidebar_fingerprint = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            editor_patch_source.hash(&mut hasher);
            hasher.finish()
        };
        if sidebar_fingerprint != ctx.frame.prev_editor_macro_sidebar_fingerprint {
            let scan = editor_patch_source
                .map(scan_patch_macro_source)
                .unwrap_or_else(|| scan_patch_macro_source(""));
            let library_macros = if editor_patch_source.is_some() {
                eseqlisp::widget_render::patcher::macro_library_sidebar_entries()
            } else {
                Vec::new()
            };
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "editor-patch-macros",
                build_patch_macro_sidebar_value(&scan.locals),
            );
            rt.set_reactive(
                "SEQ",
                "editor-library-macros",
                build_library_macro_sidebar_value(&library_macros, &scan.imports),
            );
            ctx.frame.prev_editor_macro_sidebar_fingerprint = sidebar_fingerprint;
            needs_reactive_cycle = true;
        }
        // The macro sidebar's selected row mirrors the macro view open in the
        // patcher ("" = root view).
        let open_macro = ctx
            .sessions
            .instrument_edit_session
            .as_ref()
            .map(|session| session.path.as_path())
            .or_else(|| {
                ctx.sessions
                    .effect_edit_session
                    .as_ref()
                    .map(|session| session.path.as_path())
            })
            .and_then(eseqlisp::widget_render::patcher::active_macro_view_for_path)
            .unwrap_or_default();
        if open_macro != ctx.frame.prev_editor_open_macro {
            editor.runtime_mut().set_reactive(
                "SEQ",
                "editor-open-macro",
                Value::String(open_macro.clone()),
            );
            ctx.frame.prev_editor_open_macro = open_macro;
            needs_reactive_cycle = true;
        }

        if needs_reactive_cycle {
            let profile_cycle = profile_pattern_reactive_cycle;
            let cycle_total_started = Instant::now();
            let started = Instant::now();
            editor.runtime_mut().run_reactive_cycle();
            let reactive_elapsed = started.elapsed();
            let started = Instant::now();
            editor.refresh_runtime_side_effects();
            let side_effects_elapsed = started.elapsed();
            let mut refresh_seq_elapsed = Duration::ZERO;
            let mut refresh_mixer_elapsed = Duration::ZERO;
            let mut refresh_samples_elapsed = Duration::ZERO;
            if refresh_visible_sequencer_after_cycle {
                let started = Instant::now();
                editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                refresh_seq_elapsed = started.elapsed();
            }
            if refresh_visible_mixer_after_cycle {
                let started = Instant::now();
                refresh_visible_mixer_layouts(editor);
                refresh_mixer_elapsed = started.elapsed();
            }
            if refresh_visible_samples_after_cycle {
                let started = Instant::now();
                editor.refresh_visible_layouts_for_buffer_named("*samples*");
                refresh_samples_elapsed = started.elapsed();
            }
            if refresh_visible_step_after_cycle {
                editor.refresh_visible_layouts_for_buffer_named("*step*");
            }
            editor.mark_needs_redraw();
            if profile_cycle {
                eprintln!(
                    "[pattern-switch-profile][reactive-cycle] total={:.2}ms reactive={:.2}ms side_effects={:.2}ms refresh_seq={:.2}ms refresh_mixer={:.2}ms refresh_samples={:.2}ms refresh_seq_flag={} refresh_mixer_flag={} refresh_samples_flag={}",
                    duration_ms(cycle_total_started.elapsed()),
                    duration_ms(reactive_elapsed),
                    duration_ms(side_effects_elapsed),
                    duration_ms(refresh_seq_elapsed),
                    duration_ms(refresh_mixer_elapsed),
                    duration_ms(refresh_samples_elapsed),
                    refresh_visible_sequencer_after_cycle,
                    refresh_visible_mixer_after_cycle,
                    refresh_visible_samples_after_cycle,
                );
            }
        }
    }
    ui_loop_stats.note_sync(reactive_sync_started.elapsed());

    // Keep selection animation live only during playback; when paused, edits/events
    // still request redraws explicitly, but idle should stay cheap.
    if inputs.playing_now && !ctx.shared.selected_steps.lock().unwrap().is_empty() {
        editor.mark_needs_redraw();
    }

    stub_animation_cache.update_size(inputs.viewport_size);

    // Render
    if last_render_at.elapsed() >= inputs.frame_interval {
        if inputs.stub_animation_active && !editor.needs_redraw() && !inputs.sdf_animation_active {
            if let Some(tiled_frame) = stub_animation_cache.frame() {
                let render_started = Instant::now();
                let render_status = backend
                    .render_tiled(tiled_frame)
                    .map_err(|_| "render failed")?;
                ui_loop_stats.note_frame(Duration::ZERO, render_started.elapsed());
                if render_status == TiledRenderStatus::Presented {
                    *last_render_at = Instant::now();
                    return Ok(TickFlow::Continue);
                }
                *last_render_at = Instant::now();
            }
        }
    }

    if editor.needs_redraw() && last_render_at.elapsed() >= inputs.frame_interval {
        let frame_build_started = Instant::now();
        let tiled_frame = eseqlisp::frame::build_tiled_render_frame_borderless(
            &mut editor,
            inputs.cols,
            inputs.rows,
        );
        let frame_build_elapsed = frame_build_started.elapsed();
        let render_started = Instant::now();
        let render_status = backend
            .render_tiled(&tiled_frame)
            .map_err(|_| "render failed")?;
        let render_elapsed = render_started.elapsed();
        ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed);
        match render_status {
            TiledRenderStatus::Presented => {
                editor.clear_needs_redraw();
                if backend.agent_instrument_stub_animation_visible() {
                    stub_animation_cache.store(inputs.viewport_size, tiled_frame);
                } else {
                    stub_animation_cache.reset();
                }
                *last_render_at = Instant::now();
            }
            TiledRenderStatus::NotPresented => {
                eseqlisp::frame::requeue_unpresented_tiled_frame(&mut editor, &tiled_frame);
                *last_render_at = Instant::now();
            }
        }
    }

    if editor.should_quit() {
        return Ok(TickFlow::Quit);
    }
    Ok(TickFlow::Continue)
}
