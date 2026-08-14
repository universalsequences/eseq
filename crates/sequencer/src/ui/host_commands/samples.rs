use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "audition-sample",
    "preview-sample",
    "stop-sample-preview",
    "reanalyze-sample",
    "load-sample-into-track",
    "convert-track-to-sampler",
];

#[allow(clippy::too_many_lines)]
pub(super) fn handle(
    name: &str,
    payload: Value,
    mut app: &mut app::App,
    mut editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let state = ctx.shared.state.clone();
    let lg_raw = ctx.shared.lg_raw;
    let current_track = ctx.shared.current_track.clone();
    let selected_steps = ctx.shared.selected_steps.clone();
    match name {
        "audition-sample" => {
            let path_str = extract_path_from_payload(&payload);
            eprintln!(
                "sample-host-command: audition-sample payload={payload:?}; extracted_path={path_str:?}"
            );
            if let Some(path_str) = path_str {
                if app.tracks.is_empty() {
                    editor.handle_host_event(HostEvent::Status(
                        "Add a track before auditioning samples".to_string(),
                    ));
                    return;
                }
                let path = Path::new(&path_str);
                let Some(track) = current_track_for_app(&mut app, &current_track)
                else {
                    editor.handle_host_event(HostEvent::Status(
                        "Add a track before auditioning samples".to_string(),
                    ));
                    return;
                };
                match load_or_convert_sampler_track(
                    &mut app,
                    &mut editor,
                    &state,
                    &current_track,
                    &mut *ctx.track_names,
                    &selected_steps,
                    lg_raw,
                    track,
                    Some(path),
                ) {
                    Ok(result) => {
                        let status = result.reset_summary.map_or_else(
                            || format!("Audition: {}", result.name),
                            |summary| {
                                host_commands::instrument_swap_status(
                                    "sampler", summary,
                                )
                            },
                        );
                        editor.handle_host_event(HostEvent::Status(status));
                    }
                    Err(e) => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Error loading sample: {e}"
                        )));
                    }
                }
            }
        }
        // Browser preview: play the file straight off the audio callback's
        // preview mixer (no track, no graph, not captured into exports).
        "preview-sample" => {
            let Some(path_str) = extract_path_from_payload(&payload) else {
                editor.handle_host_event(HostEvent::Status(
                    "Choose a sample file to preview".to_string(),
                ));
                return;
            };
            let path = Path::new(&path_str);
            match eseqlisp::audio::sample::load_wav_file(path) {
                Ok(decoded) => {
                    let channels = decoded.channels.max(1) as usize;
                    let mut stereo = Vec::with_capacity(decoded.frames * 2);
                    for frame in 0..decoded.frames {
                        let base = frame * channels;
                        let l = decoded.samples.get(base).copied().unwrap_or(0.0);
                        let r = if channels > 1 {
                            decoded.samples.get(base + 1).copied().unwrap_or(l)
                        } else {
                            l
                        };
                        stereo.push(l);
                        stereo.push(r);
                    }
                    sequencer::audio::preview::play(
                        std::sync::Arc::new(stereo),
                        decoded.sample_rate,
                    );
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "browser-preview-playing", Value::Bool(true));
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.mark_needs_redraw();
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&path_str);
                    editor.handle_host_event(HostEvent::Status(format!("Preview: {name}")));
                }
                Err(e) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error previewing sample: {e}"
                    )));
                }
            }
        }
        "stop-sample-preview" => {
            sequencer::audio::preview::stop();
            let rt = editor.runtime_mut();
            rt.set_reactive("SEQ", "browser-preview-playing", Value::Bool(false));
            rt.set_reactive("SEQ", "browser-preview-playhead", Value::Number(0.0));
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "reanalyze-sample" => {
            let Some(track) = current_track_for_app(&mut app, &current_track) else {
                editor.handle_host_event(HostEvent::Status(
                    "No sample loaded on this track".to_string(),
                ));
                return;
            };
            let Some(path) = app
                .sampler_paths
                .get(track)
                .and_then(|path| path.as_ref())
                .cloned()
            else {
                editor.handle_host_event(HostEvent::Status(
                    "No sample loaded on this track".to_string(),
                ));
                return;
            };
            match sequencer::instruments::sampler::load_wav_buffer(lg_raw, &path) {
                Ok(loaded) => {
                    app.submit_sample_analysis(&loaded);
                    let new_buffer_id = loaded.buffer_id;
                    let sample_rate = loaded.sample_rate;
                    app.graph_controller().send_sample_to_all_voices(
                        track,
                        new_buffer_id,
                        sample_rate,
                    );
                    app.graph.track_buffer_ids[track] = new_buffer_id;
                    app.graph.track_sample_rates[track] = sample_rate;
                    let sample_name = app.tracks[track].clone();
                    app.register_loaded_sample_path(
                        &sample_name,
                        new_buffer_id,
                        path.clone(),
                    );
                    app.publish_sampler_analysis_runtime(track);
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "instrument-panel",
                        build_instrument_panel_value(&app, track, &selected_steps),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(
                        "Re-analyzing sample".to_string(),
                    ));
                }
                Err(error) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error re-analyzing sample: {error}"
                    )));
                }
            }
        }
        "load-sample-into-track" | "convert-track-to-sampler" => {
            let path_str = extract_path_from_payload(&payload);
            let track = extract_usize_from_payload(&payload, "track");
            let preserve_browser_context =
                extract_bool_from_payload(&payload, "preserve-browser-context");
            eprintln!(
                "sample-host-command: load-sample-into-track payload={payload:?}; extracted_path={path_str:?}; extracted_track={track:?}; preserve_browser_context={preserve_browser_context}"
            );
            match (track, path_str) {
                (Some(track), Some(path_str)) => {
                    if preserve_browser_context {
                        preserve_sample_browser_context_for_loaded_sample(
                            &mut editor,
                            &path_str,
                        );
                    }
                    let path = Path::new(&path_str);
                    match load_or_convert_sampler_track(
                        &mut app,
                        &mut editor,
                        &state,
                        &current_track,
                        &mut *ctx.track_names,
                        &selected_steps,
                        lg_raw,
                        track,
                        Some(path),
                    ) {
                        Ok(result) => {
                            let status = result.reset_summary.map_or_else(
                                || {
                                    format!(
                                        "Loaded sample on track {}: {}",
                                        track + 1,
                                        result.name
                                    )
                                },
                                |summary| {
                                    format!(
                                        "{}; loaded {}",
                                        host_commands::instrument_swap_status(
                                            "sampler", summary,
                                        ),
                                        result.name
                                    )
                                },
                            );
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                        Err(e) => {
                            if preserve_browser_context {
                                preserve_sample_browser_context_for_loaded_sample(
                                    &mut editor,
                                    "",
                                );
                            }
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error loading sample: {e}"
                            )));
                        }
                    }
                }
                _ => {
                    editor.handle_host_event(HostEvent::Status(
                        "Sample drop is missing a track or path".to_string(),
                    ));
                }
            }
        }
        _ => {}
    }
}
