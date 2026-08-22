use crate::*;

pub(super) const COMMANDS: &[&str] = &["edit-sampler-slice"];

pub(super) fn handle(
    _name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Value::Map(map) = payload else { return };
    let Some(track) = map_usize(&map, "track") else {
        return;
    };
    let rack_slot = map_number(&map, "rack-slot")
        .filter(|value| *value >= 0.0)
        .map(|value| value as usize);
    let Some(operation) = map_string(&map, "operation") else {
        return;
    };
    if operation == "commit" {
        let gesture = map_string(&map, "gesture")
            .unwrap_or_else(|| "sampler-slice".to_string());
        app::edit::finish_sampler_slice_gesture(app, track, rack_slot, &gesture);
        app.publish_all_sampler_analysis_runtime();
        return;
    }
    let index = map_usize(&map, "index");
    let Some(time) = map_number(&map, "time").filter(|value| value.is_finite()) else {
        return;
    };
    let gesture = map_string(&map, "gesture").unwrap_or_else(|| "sampler-slice".to_string());
    let label = map_string(&map, "label").unwrap_or_else(|| "Edit sampler slice".to_string());

    // Marker indices address the list the panel rendered, so sensitivity has to
    // resolve exactly the way the panel resolved it (p-locks and rack macros
    // included) or an edit lands on a neighbouring marker.
    let plock_step = ctx.shared.selected_steps.lock().unwrap().iter().copied().min();
    let target = if let Some(slot_idx) = rack_slot {
        let snapshot = app.state.latest_scheduler_snapshot();
        let Some(rack) = snapshot
            .tracks
            .get(track)
            .and_then(|track| track.rack_track.as_ref())
        else {
            return;
        };
        let Some(slot) = rack.slots.get(slot_idx) else {
            return;
        };
        let Some((buffer_id, sample_name, _)) = slot.sample_id.as_ref() else {
            return;
        };
        let path = app
            .sample_buffer_path_registry
            .get(buffer_id)
            .or_else(|| app.sample_path_registry.get(sample_name));
        let Some(hash) = path.and_then(|path| {
            sequencer::analysis::sample_path_hash(&path.to_string_lossy())
        }) else {
            return;
        };
        let sensitivity = state_values::rack_panel::rack_slot_param_value(
            rack,
            slot_idx,
            slot,
            &sequencer::effects::EffectDescriptor::builtin_sampler(),
            sequencer::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY,
            plock_step,
        );
        (*buffer_id, hash, sensitivity)
    } else {
        let Some(&buffer_id) = app.graph.track_buffer_ids.get(track) else {
            return;
        };
        let Some(hash) = app
            .sampler_path_for_track(track)
            .as_ref()
            .and_then(|path| sequencer::analysis::sample_path_hash(&path.to_string_lossy()))
        else {
            return;
        };
        let Some(slot) = app.state.pattern.instrument_slots.get(track) else {
            return;
        };
        let desc = app
            .graph
            .instrument_descriptors
            .get(track)
            .cloned()
            .unwrap_or_else(sequencer::effects::EffectDescriptor::builtin_sampler);
        let Some(sensitivity) = state_values::sampler_slice_sensitivity(slot, &desc, plock_step)
        else {
            return;
        };
        (buffer_id, hash, sensitivity)
    };
    let Some(table) = app.sample_analysis.cache().table(target.0) else {
        return;
    };
    let detected = table.slice_starts(target.2).collect::<Vec<_>>();
    let frame = (time.max(0.0) * table.sample_rate as f64).round() as u32;
    let sample_len = table.sample_len_frames;
    let hash = target.1;
    let result = app::edit::apply_coalesced_sampler_slice_mutation(
        app,
        track,
        rack_slot,
        &gesture,
        &label,
        |stored| {
            if stored.as_ref().is_some_and(|edits| edits.sample_hash != hash) {
                *stored = None;
            }
            let edits = stored.get_or_insert_with(|| {
                sequencer::analysis::SamplerSliceEdits::for_sample_hash(hash.clone())
            });
            use app::edit::SamplerSliceMutation::{Applied, NoOp, Revert};
            match operation.as_str() {
                "add" => {
                    if edits.add(frame, &detected, sample_len) {
                        Applied
                    } else {
                        NoOp
                    }
                }
                "move" => match index.and_then(|index| {
                    edits
                        .resolved(&detected, sample_len)
                        .get(index)
                        .copied()
                        .map(|source| (index, source))
                }) {
                    Some((_, source)) if source == frame => Revert,
                    Some((index, _)) if edits.move_index(
                        index,
                        frame,
                        &detected,
                        sample_len,
                    ) => Applied,
                    _ => NoOp,
                },
                "delete" => {
                    if index.is_some_and(|index| {
                        edits.delete_index(index, &detected, sample_len)
                    }) {
                        Applied
                    } else {
                        NoOp
                    }
                }
                _ => NoOp,
            }
        },
    );
    match result {
        Ok(_) => {
            if operation != "move" {
                app.publish_all_sampler_analysis_runtime();
            }
            let dirty = editor
                .runtime_mut()
                .set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(app, track, &ctx.shared.selected_steps),
                )
                .effects_dirty;
            if dirty {
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
            }
        }
        Err(error) => editor.handle_host_event(HostEvent::Error(format!(
            "sampler slice edit failed: {error:?}"
        ))),
    }
}
