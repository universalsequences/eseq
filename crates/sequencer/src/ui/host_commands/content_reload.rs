//! Explicit "pick up what changed on disk" commands for instruments and
//! effects written outside the app (eseq-63j4.4). The Lisp watcher already
//! re-lists the browser and rebuilds custom panels when a user-tier folder
//! changes; these cover what it deliberately does not do on its own —
//! recompiling DSP that is running on a track — plus a manual rescan for
//! when the watcher is unavailable.

use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "reload-content-library",
    "reload-instrument-from-disk",
    "reload-effect-from-disk",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let current_track = ctx.shared.current_track.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    match name {
        "reload-content-library" => {
            let message = if rescan_content_library(editor) {
                "Rescanned instruments and effects".to_string()
            } else {
                "Rescanned instruments and effects; a custom panel failed to load (see log)"
                    .to_string()
            };
            editor.handle_host_event(HostEvent::Status(message));
            ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        "reload-instrument-from-disk" => {
            let track = extract_usize_from_payload(&payload, "track")
                .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
            match app.reload_custom_instrument_from_disk(track) {
                Ok(instrument) => {
                    rescan_content_library(editor);
                    let state = ctx.shared.state.clone();
                    sync_after_instrument_track_apply_with_selection(
                        app,
                        editor,
                        &state,
                        track,
                        &current_track,
                        &mut *ctx.track_names,
                        &ctx.shared.track_pan_ids,
                        &ctx.shared.record_armed,
                        &ctx.shared.selected_steps,
                        &ctx.shared.accumulator_names,
                        &ctx.meters.cached_track_peak_levels,
                        &ctx.meters.cached_bus_peak_levels,
                        &ui_epoch,
                        ctx.shared.lg_raw,
                        true,
                    );
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Reloaded instrument from disk: {}",
                        display_instrument_name(&instrument)
                    )));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                    "Reload instrument failed: {error}"
                ))),
            }
        }
        "reload-effect-from-disk" => {
            let track = extract_usize_from_payload(&payload, "track")
                .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
            let Some(slot) = extract_usize_from_payload(&payload, "slot") else {
                editor.handle_host_event(HostEvent::Error(
                    "Reload effect needs an effect slot".to_string(),
                ));
                return;
            };
            match saved_effect_in_slot(app, track, slot) {
                Ok(effect) => {
                    rescan_content_library(editor);
                    // Same name into the same slot: the compile runs off the
                    // UI thread and, once applied, keeps every parameter
                    // whose name survived (`apply_effect_to_slot`).
                    app.ui.cursor_track = track;
                    app.start_effect_compile(&effect, slot);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Reloading effect from disk: {effect}"
                    )));
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => editor
                    .handle_host_event(HostEvent::Error(format!("Reload effect failed: {error}"))),
            }
        }
        _ => {}
    }
}

/// The saved (file-backed) effect in `slot` of `track`'s chain.
fn saved_effect_in_slot(app: &app::App, track: usize, slot: usize) -> Result<String, String> {
    let name = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|chain| chain.get(slot))
        .map(|desc| desc.name.clone())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("track {} has no effect in slot {}", track + 1, slot + 1))?;
    if !sequencer::lisp_host::effect_source_path(&name).is_file() {
        return Err(format!("'{name}' is not a saved effect with a dsp.lisp"));
    }
    Ok(name)
}
