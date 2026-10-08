use crate::*;

/// Values computed earlier in the loop iteration that the tick consumes.
pub(crate) struct TickInputs {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

pub(crate) enum TickFlow {
    /// Move on to the next loop iteration.
    Continue,
    /// The editor requested shutdown; leave the event loop.
    Quit,
}

/// Post-event reactive sync + render: diffs sequencer/transport state against
/// the previous frame, republishes reactives, and renders when dirty.
/// Production control/UI synchronization, independent of presentation. Keeping
/// this separate lets headless input probes measure the actual per-frame work.
#[allow(clippy::too_many_lines)]
pub(crate) fn sync_reactive_tick(
    mut app: &mut app::App,
    mut editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    ui_loop_stats: &mut UiLoopStats,
) {
    host_commands::export::poll(editor);
    // Rack-slot instrument params resolve by name in the scheduler
    // (`seq-emit :params`), which needs the registry's descriptors.
    let registry = &app.editor.engine_registry;
    ctx.shared.state.sync_engine_instrument_descriptors(registry.epoch(), || {
        registry.instrument_descriptors().to_vec()
    });
    // A published snapshot that changed a track's parameter set (instrument
    // swap, FX / MIDI FX chain, rack macros) re-judges open `plock` names.
    if super::param_words::refresh_param_word_source() {
        editor.mark_needs_redraw();
    }
    poll_pending_compile_status(
        &mut app,
        &mut editor,
        &ctx.shared.fx_epoch,
        &ctx.shared.ui_epoch,
    );

    // Theme changes do not mutate the project or bump its UI epoch. The host
    // kinds re-push the tinted track and variant colors on a tint change
    // (their model revision carries both keys); the sound palette rows
    // publish tinted RGB, so republish them when the variant tint moves.
    let variant_tint = eseqlisp::theme::variant_display_key();
    if Some(variant_tint) != ctx.frame.prev_variant_tint {
        ctx.frame.prev_variant_tint = Some(variant_tint);
        ctx.frame.sound_palette.invalidate_published_colors();
        ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }

    // Inline runtime values are polled while building a render frame. The UI
    // is otherwise event-driven, so scheduler-owned channel changes must ask
    // for a frame or a process-driven slider remains still until unrelated
    // input redraws it (and appears to snap under the pointer).
    let process_channel_values_version =
        ctx.shared.state.process_channel_values_version();
    if process_channel_values_version != ctx.frame.prev_process_channel_values_version {
        ctx.frame.prev_process_channel_values_version = process_channel_values_version;
        if editor.has_visible_inline_runtime_bindings() {
            editor.mark_needs_redraw();
        }
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
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
        editor.mark_needs_redraw();
    }

    // Live param printing: while playing+recording, an armed target writes
    // onto each trigger step the playhead passes. Step targets push targeted
    // invalidations; instrument targets update atomic p-lock storage directly.
    // Both ride the open "Record take" transaction like live note recording.
    // The held controls show the latch through the host kinds.
    if tick_step_print(&mut app, ctx.shared).printed {
        app.mark_recording_take_changed();
        editor.mark_needs_redraw();
    }

    // 2. Sync reactive state AFTER events
    let ct = current_track_for_app(&mut app, &ctx.shared.current_track).unwrap_or(0);
    let sampler_playhead_wanted = ctx.frame.host_kinds.wants_sampler_playhead();
    sync_watched_sampler_voices(
        &app,
        sampler_playhead_wanted.then_some(ct),
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
        let playhead = ctx.shared.state.transport.track_playheads[ct].load(Ordering::Relaxed);
        let epoch = ctx.shared.state.transport.pattern_epoch.load(Ordering::Relaxed);
        let current_track_playhead_changed = playhead != ctx.frame.prev_playhead;
        poll_observed_meters(&app, ctx, ct);
        let mut needs_reactive_cycle = false;
        // The drum rack pad lights hold while a member's note sounds, so the
        // rack members' active notes are scanned every tick (hidden too);
        // other tracks need no 128-note scan (`track.active-notes` reads its
        // own, host_kinds/graphs.rs).
        let mut track_active_notes: Vec<Vec<sequencer::sequencer::ActiveNoteActivity>> =
            Vec::new();
        for group in app.groups.iter().filter(|group| group.is_rack()) {
            for &track in group.members.iter().filter(|&&track| track < app.tracks.len()) {
                if track_active_notes.len() <= track {
                    track_active_notes.resize_with(track + 1, Vec::new);
                }
                track_active_notes[track] = ctx.shared.state.active_note_activity(track);
            }
        }
        // Track switch — rebuild everything
        if ct != ctx.frame.prev_current_track && !app.tracks.is_empty() {
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
            // The device panels lay out from the kinds
            // (eseq.effects.panel-data), which follow the current track.
            sync_sidebar_browser(&app, ct);
            ctx.frame.prev_current_track = ct;
            ctx.frame.prev_pattern_epoch = epoch;
            needs_reactive_cycle = true;
        }

        // Track-groups reconcile: pull native-mutated groups (collapse toggle,
        // group create) into app.groups; the host kinds publish them.
        {
            let groups = ctx.shared.track_groups.lock().unwrap();
            if *groups != ctx.frame.prev_groups {
                app.groups.clone_from(&groups);
                ctx.frame.prev_groups.clone_from(&groups);
            }
        }

        if playing != ctx.frame.prev_playing {
            // A transport flip means the user is playing/recording now: drop
            // any widget focus left by an earlier click, so global shortcuts
            // (record, live keys) stop landing in that widget.
            editor.blur_all_widget_focus();
            ctx.frame.prev_playing = playing;
            needs_reactive_cycle = true;
            if !app.tracks.is_empty() {
                drop_stale_plock_preview(ctx, ct);
            }
        }
        if bpm != ctx.frame.prev_bpm {
            app.push_all_delay_bpm();
            ctx.frame.prev_bpm = bpm;
            needs_reactive_cycle = true;
        }
        // Poll the event count every UI tick, independently of the smoothed
        // percentage: `engine.overloaded` (the host kinds) shows both edges,
        // so reopening the transport cannot retain an expired warning.
        let deadline_misses = ctx
            .shared
            .state
            .transport
            .audio_deadline_misses
            .load(Ordering::Relaxed);
        ctx.frame
            .cpu_overload
            .update(deadline_misses, Instant::now());
        app.ui.master_recording = ctx.shared.master_recording.load(Ordering::Acquire);
        // Song-mode bindings (docs/song-mode-spec.md 12): diff-published each
        // frame; the arrangement is re-read only on committed-song revision
        // change, and the lane surfaces derived from it diff by value.
        // The render-rate song position drives the transport readout and the
        // arrangement playhead (kind fields, pushed while observed).
        // Clip selection is dormant while the timeline is off screen (takes
        // spec 16.6), so the binding needs the view state before it resolves.
        app.set_arrangement_view_visible(editor_has_visible_buffer(&editor, "*arrangement*"));
        // Sound binding (takes spec 16.2): keep the live device mirror on the
        // bound source before anything reads it. This is where a song row
        // transition (rule 2) re-binds the panel and the monitor sound, and
        // where a lane released by a session save-back is reloaded.
        app.sync_track_sound_bindings();
        // A binding move rewrites the mirror's devices without touching the
        // pattern epoch, so the panels would keep showing the old source's
        // knobs (and the old badge) until some unrelated edit republished
        // them. Drive the same model sync a device change does (the host
        // kinds re-key the devices and their params).
        if app.sound_binding_epoch != ctx.frame.prev_sound_binding_epoch {
            ctx.frame.prev_sound_binding_epoch = app.sound_binding_epoch;
            ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        }
        needs_reactive_cycle |= crate::retrospective::sync(editor.runtime_mut(), &app);
        let pattern_glyphs_visible = editor.has_visible_widget_source("sound-glyph", "pattern-glyph:");
        if sync_sound_palette(&app, &mut ctx.frame.sound_palette, pattern_glyphs_visible) {
            editor.mark_needs_redraw();
        }
        // Drum-rack pad lights (eseq-4b5.16). The flags are read every tick —
        // reading is what consumes the audio thread's trigger latch, so it must
        // not be skipped. Host kinds read them as `pad.triggered`, gated by
        // that field's observers.
        ctx.frame.rack_pad_triggers = read_rack_pad_trigger_flags(
            &app,
            &ctx.shared.state,
            &track_active_notes,
            &mut ctx.frame.rack_pad_triggered_at,
            Instant::now(),
        );
        // The modulator envelopes and the modulation sources' phases reach
        // the panels as eseq.kinds fields (device.modulator-phase / -level,
        // param.mod-phase), which read the meter cache themselves.
        // The tracker and the expanded editors light their playing steps
        // from the kinds (track.playhead-row, step.playing); only the
        // snapshot the host commands compare against is kept here.
        ctx.frame.prev_track_playheads = track_playheads_snapshot(&ctx.shared.state, &app);
        ctx.frame.prev_playhead = playhead;
        if current_track_playhead_changed && !app.tracks.is_empty() {
            drop_stale_plock_preview(ctx, ct);
        }
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
                accumulator_names: &ctx.shared.accumulator_names,
            },
        ) {
            needs_reactive_cycle = true;
        }
        // Edit-focus refresh (clip-edit-target spec 3): project the
        // App-resolved target into the cell the `seq-piano-roll-action`
        // native reads. The piano roll's own fields are host kinds
        // (`piano-roll`, `note`), which follow the focus themselves.
        {
            let focus = PianoRollFocusSpec::from_focus(app.track_edit_focus(ct));
            let focus_changed = {
                let mut cell = ctx.shared.piano_roll_focus.lock().unwrap();
                std::mem::replace(&mut *cell, focus) != focus
            };
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
        }
        // Kind instances (instance-kinds spec §5): an instance edit, undo/redo,
        // project open or a (re)registered kind publishes each instance's
        // sequencer under its id and mirrors the list into the VM's records.
        // The UI VM's own kind set is part of the key: whether a record can
        // exist depends on this VM holding the schema, and another runtime
        // registering an identical definition first leaves the host
        // registry version unchanged when the UI VM catches up.
        let ui_kinds = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            editor.runtime_mut().instance_kind_ids().hash(&mut hasher);
            hasher.finish()
        };
        let instance_key = (
            app.instances.revision,
            sequencer::lisp_host::kind_registry_version(),
            ui_kinds,
            app.instances.generation,
        );
        if instance_key != ctx.frame.prev_instance_key {
            // A replaced list (project open / new project) starts from fresh
            // records: the previous project's view state never carries over.
            let replaced = instance_key.3 != ctx.frame.prev_instance_key.3;
            if replaced {
                needs_reactive_cycle |=
                    sequencer::lisp_host::drop_all_instance_records(editor.runtime_mut());
            }
            ctx.frame.prev_instance_key = instance_key;
            app.publish_instance_sequencers();
            needs_reactive_cycle |=
                sequencer::lisp_host::sync_instance_records(editor.runtime_mut(), &app.instances);
            // Per-instance view buffers and step tabs (spec §7), and with
            // a replaced list, fresh ones.
            needs_reactive_cycle |= host_commands::instances::sync_instance_views(
                editor,
                &app.instances,
                replaced,
            );
        }
        // Tracked graph reads (`graph-edge-value` & co., instance-kinds spec
        // §6): Lisp `graph-*` writes dirty their readers synchronously; this
        // sweep catches everything else that can move a resolved graph value.
        // Generations are the resolved values, so unchanged reads stay clean.
        let graph_read_key = (
            ctx.shared.state.scheduler_snapshot_version(),
            ctx.shared.state.published_sequencers_version(),
            ctx.shared.state.current_pattern_index(),
        );
        if graph_read_key != ctx.frame.prev_graph_read_key {
            ctx.frame.prev_graph_read_key = graph_read_key;
            needs_reactive_cycle |= sequencer::lisp_host::queue_graph_read_invalidations(
                editor.runtime_mut(),
                &ctx.shared.state,
            );
        }
        let mirror_epoch = app.song_row_mirror_epoch;
        if (epoch != ctx.frame.prev_pattern_epoch
            || mirror_epoch != ctx.frame.prev_song_row_mirror_epoch)
            && !app.tracks.is_empty()
        {
            let profile_switch = pattern_switch_profile_enabled();
            let profile_total_started = Instant::now();
            let old_pattern_epoch = ctx.frame.prev_pattern_epoch;
            sync_shared_track_collapsed(&ctx.shared.track_collapsed, &app);
            refresh_track_names_cache(&mut *ctx.track_names, &app);
            sync_scene_slot_state(editor.runtime_mut(), &ctx.shared.state);
            *ctx.shared.accumulator_names.lock().unwrap() = build_accumulator_names(&app);
            drop_stale_plock_preview(ctx, ct);
            sync_sidebar_browser(&app, ct);
            if profile_switch {
                eprintln!(
                    "[pattern-switch-profile][epoch-sync] total={:.2}ms epoch {}->{}",
                    duration_ms(profile_total_started.elapsed()),
                    old_pattern_epoch,
                    epoch,
                );
            }
            ctx.frame.prev_pattern_epoch = epoch;
            ctx.frame.prev_song_row_mirror_epoch = mirror_epoch;
            ctx.frame.prev_track_button_states = track_button_state_snapshot(&ctx.shared.state);
            needs_reactive_cycle = true;
            // The layout refreshes skip a buffer no tile shows.
            refresh_visible_mixer_after_cycle = true;
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
            let multi_track_selection = {
                let guard = ctx.shared.active_delete_target.lock().unwrap();
                match guard.as_ref() {
                    Some(ActiveDeleteTarget::TrackSteps { tracks }) => tracks.clone(),
                    _ => Vec::new(),
                }
            };
            // A multi-track step selection highlights non-current tracks only
            // while its target is armed; once it drops, republish those rows.
            if ctx.frame.prev_multi_track_selection != multi_track_selection {
                let track_count = ctx.shared.state.active_track_count();
                for track in &ctx.frame.prev_multi_track_selection {
                    if multi_track_selection.contains(track) || *track >= track_count {
                        continue;
                    }
                    let num_steps = ctx.shared.state.pattern.track_params[*track]
                        .get_num_steps()
                        .min(sequencer::sequencer::MAX_STEPS);
                    ctx.shared.ui_invalidations.push(UiInvalidation::StepSelection {
                        track: *track,
                        changed_steps: (0..num_steps).collect(),
                    });
                }
                ctx.frame.prev_multi_track_selection = multi_track_selection;
            }
            needs_reactive_cycle = true;
        }
        let ui_ep = ctx.shared.ui_epoch.load(Ordering::Relaxed);
        if ui_ep != ctx.frame.prev_ui_epoch {
            if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                eprintln!(
                    "[ui-trace][metal_seq] ui_epoch {}->{} ct={}",
                    ctx.frame.prev_ui_epoch,
                    ui_ep,
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
            sync_shared_track_collapsed(&ctx.shared.track_collapsed, &app);
            refresh_track_names_cache(&mut *ctx.track_names, &app);
            if app.tracks.is_empty() {
                sync_scene_slot_state(editor.runtime_mut(), &ctx.shared.state);
            } else {
                *ctx.shared.accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                drop_stale_plock_preview(ctx, ct);
            }
            // Sync recording state
            let rec_on = ctx.shared.recording.load(Ordering::Relaxed);
            let master_rec_on = ctx.shared.master_recording.load(Ordering::Acquire);
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
            // Sync to app for TUI recording logic
            app.ui.recording = rec_on;
            app.ui.master_recording = master_rec_on;
            for (i, a) in armed.iter().enumerate() {
                if i < app.graph.record_armed.len() {
                    app.graph.record_armed[i] = *a;
                }
            }
            refresh_visible_sequencer_after_cycle = true;
            refresh_visible_mixer_after_cycle |= record_armed_changed || track_buttons_changed;
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
                    needs_reactive_cycle = true;
                }
                ctx.frame.prev_sampler_analysis_key = analysis_key;
            }
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
                present_editor(editor.runtime_mut(), |e| {
                    e.active_macro.clone_from(&editor_macro_action.0);
                    e.active_macro_action.clone_from(&editor_macro_action.1);
                });
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
        let editor_patch_path = ctx
            .sessions
            .instrument_edit_session
            .as_ref()
            .map(|session| session.path.as_path())
            .or_else(|| {
                ctx.sessions
                    .effect_edit_session
                    .as_ref()
                    .map(|session| session.path.as_path())
            });
        let sidebar_fingerprint = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            editor_patch_source.hash(&mut hasher);
            editor_patch_path.hash(&mut hasher);
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
            let assets = eseqlisp::widget_render::patcher::asset_sidebar_entries(editor_patch_path);
            present_editor_sidebar(editor.runtime_mut(), |sidebar| {
                sidebar.patch_macros = patch_macro_sidebar(scan.locals);
                sidebar.library_macros = library_macro_sidebar(library_macros, &scan.imports);
                sidebar.assets = asset_sidebar(assets);
            });
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
            present_editor(editor.runtime_mut(), |e| {
                e.open_macro.clone_from(&open_macro)
            });
            ctx.frame.prev_editor_open_macro = open_macro;
            needs_reactive_cycle = true;
        }

        // The sidebar's asset inspector mirrors the single selected
        // file-backed tensor node. The render pass maintains the reference
        // per patcher key; metadata is re-read (through the mtime-keyed
        // header cache) only when the reference changes.
        let selected_asset =
            editor_patch_path.and_then(eseqlisp::widget_render::patcher::selected_asset_for_path);
        if selected_asset != ctx.frame.prev_editor_selected_asset {
            let info = selected_asset.as_deref().map(|reference| {
                let draft_root = editor_patch_path.and_then(std::path::Path::parent);
                crate::presented::AssetInfo {
                    reference: reference.to_string(),
                    // Unresolvable or invalid: the inspector still shows the
                    // reference with no metadata rows.
                    metadata: eseqlisp::editor::asset_metadata(reference, draft_root),
                }
            });
            present_editor_sidebar(editor.runtime_mut(), |sidebar| {
                sidebar.selected_asset = info
            });
            ctx.frame.prev_editor_selected_asset = selected_asset;
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
    // Host kinds (eseq.kinds): registry, model fields, observed live fields.
    let meters = super::host_kinds::KindsMeters {
        tracks: &ctx.meters.cached_track_peak_levels,
        buses: &ctx.meters.cached_bus_peak_levels,
        master: (
            ctx.meters.cached_peak_l_level,
            ctx.meters.cached_peak_r_level,
        ),
        cpu_load: f32::from_bits(ctx.meters.cached_cpu_load_bits) as f64,
        mod_ports: &ctx.meters.cached_mod_port_levels,
        overloaded: ctx.frame.cpu_overload.displayed(),
        pad_triggers: &ctx.frame.rack_pad_triggers,
        mod_display: &ctx.meters.cached_mod_display_values,
        modulator_phases: &ctx.meters.cached_modulator_phases,
        modulator_levels: &ctx.meters.cached_modulator_levels,
    };
    let host_kinds = &mut ctx.frame.host_kinds;
    host_kinds.set_plock_preview(ctx.gesture.preview_plock_variant.as_ref());
    if host_kinds.sync(app, editor.runtime_mut(), ctx.shared, &meters) {
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
    ui_loop_stats.note_sync(reactive_sync_started.elapsed());
}

/// Drop a p-lock variant preview (`GestureState::preview_plock_variant`)
/// that no longer matches the current track with no step selected.
fn drop_stale_plock_preview(ctx: &mut LoopCtx<'_>, ct: usize) {
    if ctx.gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
        *track != ct || !ctx.shared.selected_steps.lock().unwrap().is_empty()
    }) {
        ctx.gesture.preview_plock_variant = None;
    }
}

/// Poll the meter caches the host kinds read (`KindsMeters`), each only
/// while a kind field observes it (`HostKinds::wants_*`, as of the last
/// sync; docs/kind-bindings-spec.md D3): the track, bus and master peaks,
/// the modulator envelopes, the mod port levels and the modulation display
/// sample. An observed cache polls at the meter cadence, and at once when
/// it is newly observed or sized for another topology. With nothing
/// observed nothing is read, and the modulation sample releases its
/// audio-graph watchlist.
pub(crate) fn poll_observed_meters(app: &app::App, ctx: &mut LoopCtx<'_>, ct: usize) {
    let demand = MeterDemand::of(&ctx.frame.host_kinds);
    let was = std::mem::replace(&mut ctx.frame.prev_meter_demand, demand);
    let meters = &mut *ctx.meters;
    let meter_polled = meters.last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL;
    let due = |wanted: bool, was_wanted: bool, resized: bool| {
        wanted && (meter_polled || !was_wanted || resized)
    };
    let lg = app.graph.lg;
    if due(demand.master, was.master, false) {
        meters.cached_peak_l_level = meter_display_level(f32::from_bits(
            ctx.shared.state.transport.peak_l.load(Ordering::Relaxed),
        ));
        meters.cached_peak_r_level = meter_display_level(f32::from_bits(
            ctx.shared.state.transport.peak_r.load(Ordering::Relaxed),
        ));
    }
    if due(
        demand.tracks,
        was.tracks,
        meters.cached_track_peak_levels.len() != app.tracks.len(),
    ) {
        meters.cached_track_peak_levels = read_track_peak_levels(lg, &app.graph.track_node_ids);
    }
    if due(
        demand.buses,
        was.buses,
        meters.cached_bus_peak_levels.len() != app.buses.len(),
    ) {
        meters.cached_bus_peak_levels = read_bus_peak_levels(lg, &app.graph.bus_node_ids);
    }
    let modulator_tracks =
        (app.graph.track_node_ids.len()).min(app.graph.track_instrument_types.len());
    if due(
        demand.modulators,
        was.modulators,
        meters.cached_modulator_phases.len() != modulator_tracks,
    ) {
        (meters.cached_modulator_phases, meters.cached_modulator_levels) =
            read_modulator_display_values(lg, app);
    }
    if due(
        demand.mod_levels,
        was.mod_levels,
        meters.cached_mod_port_levels.track_outputs.len() != app.tracks.len(),
    ) {
        meters.cached_mod_port_levels = read_mod_port_levels(lg, app);
    }
    if meter_polled {
        meters.last_meter_poll_at = Instant::now();
    }
    // Released meters fall to silence, so a reopened one shows silence, not
    // an old peak, for the one frame before its first fresh sample.
    if was.master && !demand.master {
        meters.cached_peak_l_level = 0.0;
        meters.cached_peak_r_level = 0.0;
    }
    if was.tracks && !demand.tracks {
        meters.cached_track_peak_levels.fill(0.0);
    }
    if was.buses && !demand.buses {
        meters.cached_bus_peak_levels.fill(0.0);
    }
    if was.mod_levels && !demand.mod_levels {
        let levels = &mut meters.cached_mod_port_levels;
        levels.track_inputs.iter_mut().for_each(|inputs| inputs.fill(0.0));
        levels.track_outputs.fill(0.0);
        levels.bus_inputs.iter_mut().for_each(|(_, inputs)| inputs.fill(0.0));
    }
    // Effective (post-modulation) param values (eseq-dtx.13, generalized in
    // eseq-hpc): every effect plus the selected track's instrument
    // (eseq-6mva), whose per-voice modulators are read through the audio
    // thread's published last-triggered voice. Also sampled off-cadence
    // when `fx_epoch` moves (a freshly inserted effect is seeded with its
    // base values in the tick that builds its panel) and on a track switch
    // (never the previous instrument's modulation).
    if !ctx.frame.host_kinds.wants_mod_display() {
        // Releasing the watchlist also removes audio-thread snapshot work.
        // The last values stay for the next observer; releasing clears the
        // poll track, so that observer samples at once.
        for node in meters.watched_display_modulators.drain() {
            unsafe { sequencer::audiograph::remove_node_from_watchlist(lg.0, node); }
        }
        meters.mod_display_poll_track = None;
        return;
    }
    let mod_display_epoch = ctx.shared.fx_epoch.load(Ordering::Relaxed);
    if meter_polled
        || mod_display_epoch != meters.mod_display_poll_fx_epoch
        || Some(ct) != meters.mod_display_poll_track
    {
        meters.mod_display_poll_fx_epoch = mod_display_epoch;
        meters.mod_display_poll_track = Some(ct);
        meters.cached_mod_display_values = read_mod_display_values(
            lg,
            app,
            &ctx.shared.state,
            Some(ct),
            selected_plock_step(&ctx.shared.selected_steps),
            true,
            &mut meters.watched_display_modulators,
        );
    }
}

pub(crate) fn reactive_tick_and_render(
    app: &mut app::App,
    mut editor: &mut Editor,
    backend: &mut AppBackend,
    ctx: &mut LoopCtx<'_>,
    inputs: TickInputs,
    frame_pacer: &mut frame_pacer::FramePacer,
    ui_loop_stats: &mut UiLoopStats,
) -> Result<TickFlow, Box<dyn std::error::Error>> {
    sync_reactive_tick(app, editor, ctx, ui_loop_stats);

    if editor.needs_redraw() && frame_pacer.is_due(Instant::now()) {
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
        ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed, render_status == TiledRenderStatus::Presented);
        match render_status {
            TiledRenderStatus::Presented => {
                editor.clear_needs_redraw();
                frame_pacer.frame_finished(Instant::now());
            }
            TiledRenderStatus::NotPresented => {
                eseqlisp::frame::requeue_unpresented_tiled_frame(&mut editor, &tiled_frame);
                frame_pacer.frame_finished(Instant::now());
            }
        }
    }

    if editor.should_quit() {
        if !host_commands::intercept_unsaved_quit(app, editor, ctx) {
            return Ok(TickFlow::Quit);
        }
    }
    Ok(TickFlow::Continue)
}
