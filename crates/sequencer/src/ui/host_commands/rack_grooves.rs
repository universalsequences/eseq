use crate::*;

use sequencer::app::rack_grooves::RackGrooveExtractRequest;
use sequencer::groove::library::{
    delete_library_groove, load_library_groove, rename_library_groove,
};
use sequencer::groove::{
    GrooveChoice, GrooveExtractOptions, GROOVE_PERIOD_ONE_BAR, GROOVE_PERIOD_TWO_BARS,
    GROOVE_RESOLUTION_SIXTEENTH, GROOVE_RESOLUTION_THIRTY_SECOND,
};

/// Groove commands (docs/rack-groove-spec.md, "UI"): the drum rack panel's
/// Groove section, its Extract Groove modal, and the pool/library edits the
/// Grooves tab shares. Rack commands address a rack by its stable `GroupId`;
/// a groove is a project POOL id (`groove-id`), a library file a
/// `stem` in the user tier.
pub(super) const COMMANDS: &[&str] = &[
    "extract-rack-groove",
    "set-rack-groove",
    "set-rack-groove-amount",
    "set-rack-groove-enabled",
    "set-rack-groove-scale",
    "set-rack-clip-own-groove",
    "apply-rack-groove-to-all-clips",
    "set-rack-groove-pad-amount",
    "set-rack-groove-pad-enabled",
    "rename-rack-groove",
    "duplicate-pool-groove",
    "delete-rack-groove",
    "save-groove-to-library",
    "rename-library-groove",
    "delete-library-groove",
];

/// Commands that act on the pool or the library, not on one rack: no
/// `group-id` needed.
const POOL_COMMANDS: &[&str] = &[
    "rename-rack-groove",
    "duplicate-pool-groove",
    "delete-rack-groove",
    "save-groove-to-library",
    "rename-library-groove",
    "delete-library-groove",
];

/// What an applied groove command changed, so the caller republishes only
/// that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RackGrooveEdit {
    /// Rack config only (the groove list or selection).
    Structure,
    /// Rack config AND the source patterns (extract + quantize source).
    StructureAndPatterns,
    /// An amount knob moved; `false` when the value did not change.
    Amount(bool),
}

fn group_id(payload: &Value, name: &str) -> Result<u64, String> {
    extract_usize_from_payload(payload, "group-id")
        .map(|id| id as u64)
        .ok_or_else(|| format!("{name} needs a group id"))
}

fn groove_id(payload: &Value, name: &str) -> Result<u64, String> {
    extract_usize_from_payload(payload, "groove-id")
        .map(|id| id as u64)
        .ok_or_else(|| format!("{name} needs a groove id"))
}

/// The Extract Groove modal's choices: `bars` 1 or 2, `resolution` "1/16"
/// or "1/32", `quantize` (default on in the modal).
pub(crate) fn extract_request_from_payload(
    payload: &Value,
) -> Result<RackGrooveExtractRequest, String> {
    let period_beats = match extract_usize_from_payload(payload, "bars").unwrap_or(1) {
        1 => GROOVE_PERIOD_ONE_BAR,
        2 => GROOVE_PERIOD_TWO_BARS,
        other => return Err(format!("A groove period is 1 or 2 bars, not {other}")),
    };
    let resolution_beats = match extract_string_from_payload(payload, "resolution").as_deref() {
        None | Some("1/16") | Some("16th") => GROOVE_RESOLUTION_SIXTEENTH,
        Some("1/32") | Some("32nd") => GROOVE_RESOLUTION_THIRTY_SECOND,
        Some(other) => return Err(format!("Unknown groove resolution {other:?}")),
    };
    let name = extract_string_from_payload(payload, "name")
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| GrooveExtractOptions::default().name);
    Ok(RackGrooveExtractRequest {
        options: GrooveExtractOptions {
            name,
            period_beats,
            resolution_beats,
        },
        quantize_source: extract_bool_from_payload(payload, "quantize"),
        ..Default::default()
    })
}

/// Applies one groove command to the app. `None` when `name` is not a groove
/// command. Shared by the live handler and `metal_seq capture` setup.
pub(crate) fn apply_rack_groove_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Option<Result<RackGrooveEdit, String>> {
    if !COMMANDS.contains(&name) {
        return None;
    }
    Some(apply(name, payload, app))
}

fn apply(name: &str, payload: &Value, app: &mut app::App) -> Result<RackGrooveEdit, String> {
    if POOL_COMMANDS.contains(&name) {
        return apply_pool_command(name, payload, app);
    }
    let group = group_id(payload, name)?;
    // The clip whose groove the edit targets (`clip-id`, absent or -1 for the
    // rack's own): the buffer names the clip it shows, so a launch landing
    // between drawing and clicking cannot redirect the edit.
    let clip = extract_i32_from_payload(payload, "clip-id")
        .filter(|clip| *clip >= 0)
        .map(|clip| clip as u64);
    let target = |app: &app::App| {
        rack_of(app, group).map(|rack| rack.groove_for_clip(clip).clone())
    };
    // An edit to a clip that still follows the rack gives it its own groove
    // (copy-on-write): a new entry the buffer must learn about, so even an
    // amount drag's first step republishes the structure.
    let follows_rack = |app: &app::App| {
        clip.is_some_and(|clip| rack_of(app, group).is_some_and(|rack| rack.clip_groove(clip).is_none()))
    };
    let amount_edit = |changed: bool, forked: bool| {
        if changed && forked {
            RackGrooveEdit::Structure
        } else {
            RackGrooveEdit::Amount(changed)
        }
    };
    match name {
        "extract-rack-groove" => {
            let request = extract_request_from_payload(payload)?;
            app.extract_rack_groove_recorded(group, &request)?;
            Ok(if request.quantize_source {
                RackGrooveEdit::StructureAndPatterns
            } else {
                RackGrooveEdit::Structure
            })
        }
        // `key` is a picker key: `pool:<id>`, `factory:<stem>`,
        // `user:<stem>` or `off`. A library key copies the file's groove
        // into the pool first (copy-on-apply).
        "set-rack-groove" => {
            let key = extract_string_from_payload(payload, "key").unwrap_or_default();
            let active = match GrooveChoice::from_picker_key(&key)? {
                GrooveChoice::Off => None,
                GrooveChoice::Pool(id) => Some(id),
                GrooveChoice::Library { tier, stem } => {
                    let groove = load_library_groove(tier, &stem)
                        .map_err(|error| format!("Could not load groove {key:?}: {error}"))?;
                    app.apply_library_groove_recorded(group, clip, &groove)?;
                    return Ok(RackGrooveEdit::Structure);
                }
            };
            let unchanged = target(app).is_some_and(|settings| settings.active == active);
            if !unchanged {
                app.set_rack_active_groove_recorded(group, clip, active)?;
            }
            Ok(RackGrooveEdit::Structure)
        }
        // `amount` is "timing", "velocity" or "random"; the value is clamped
        // into the amount's range. A knob drag coalesces into one undo step.
        "set-rack-groove-amount" => {
            let amount = extract_string_from_payload(payload, "amount").unwrap_or_default();
            if !matches!(amount.as_str(), "timing" | "velocity" | "random") {
                return Err(format!("Unknown groove amount {amount:?}"));
            }
            let value = extract_f32_from_payload(payload, "value")
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("{name} needs a finite value"))?;
            let forked = follows_rack(app);
            let changed = app::edit::apply_rack_groove_amount_drag(app, group, clip, |settings| {
                match amount.as_str() {
                    "timing" => settings.timing_amount = value,
                    "velocity" => settings.velocity_amount = value,
                    "random" => settings.random_amount = value,
                    _ => {}
                }
            })?;
            Ok(amount_edit(changed, forked))
        }
        // The rack groove buffer's on/off switch (keeps the selection).
        "set-rack-groove-enabled" => {
            let enabled = extract_bool_from_payload(payload, "enabled");
            match app.set_rack_groove_enabled_recorded(group, clip, enabled) {
                Ok(()) => Ok(RackGrooveEdit::Structure),
                Err(_) if target(app).is_some_and(|settings| settings.enabled == enabled) => {
                    Ok(RackGrooveEdit::Structure)
                }
                Err(error) => Err(error),
            }
        }
        // The groove's time scale: 0.5, 1 or 2 (the buffer's Scale dropdown).
        "set-rack-groove-scale" => {
            let scale = extract_f32_from_payload(payload, "scale")
                .ok_or_else(|| format!("{name} needs a scale"))?;
            match app.set_rack_groove_scale_recorded(group, clip, scale) {
                Ok(()) => Ok(RackGrooveEdit::Structure),
                Err(_) if target(app).is_some_and(|settings| settings.scale == scale) => {
                    Ok(RackGrooveEdit::Structure)
                }
                Err(error) => Err(error),
            }
        }
        // One pad's share of the groove, 0..1 (the buffer's Amt column). A
        // drag coalesces with the rack's amount knobs into one undo step.
        "set-rack-groove-pad-amount" => {
            let pad_note = pad_note(payload, name)?;
            let value = extract_f32_from_payload(payload, "value")
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("{name} needs a finite value"))?;
            require_pad(app, group, pad_note)?;
            let forked = follows_rack(app);
            let changed = app::edit::apply_rack_groove_amount_drag(app, group, clip, |settings| {
                settings.pad_mut(pad_note).amount = value;
            })?;
            Ok(amount_edit(changed, forked))
        }
        // Include a pad in the groove or leave it straight.
        "set-rack-groove-pad-enabled" => {
            let pad_note = pad_note(payload, name)?;
            let enabled = extract_bool_from_payload(payload, "enabled");
            match app.set_rack_groove_pad_enabled_recorded(group, clip, pad_note, enabled) {
                Ok(()) => Ok(RackGrooveEdit::Structure),
                Err(_) if target(app).is_some_and(|settings| settings.pad(pad_note).enabled == enabled) => {
                    Ok(RackGrooveEdit::Structure)
                }
                Err(error) => Err(error),
            }
        }
        // Give the clip its own groove (`own` true), or return it to the
        // rack's.
        "set-rack-clip-own-groove" => {
            let clip = clip.ok_or_else(|| format!("{name} needs a clip id"))?;
            let own = extract_bool_from_payload(payload, "own");
            match app.set_rack_clip_own_groove_recorded(group, clip, own) {
                Ok(()) => Ok(RackGrooveEdit::Structure),
                Err(_) if rack_of(app, group).is_some_and(|rack| rack.clip_groove(clip).is_some() == own) => {
                    Ok(RackGrooveEdit::Structure)
                }
                Err(error) => Err(error),
            }
        }
        // The groove `clip` plays becomes the rack's, for every clip.
        "apply-rack-groove-to-all-clips" => {
            app.apply_rack_groove_to_all_clips_recorded(group, clip)?;
            Ok(RackGrooveEdit::Structure)
        }
        other => Err(format!("Unknown rack groove command {other}")),
    }
}

fn pad_note(payload: &Value, name: &str) -> Result<i32, String> {
    extract_i32_from_payload(payload, "pad-note")
        .ok_or_else(|| format!("{name} needs a pad note"))
}

fn rack_of(app: &app::App, group: u64) -> Option<&sequencer::project::ProjectRackConfig> {
    app.groups
        .iter()
        .find(|candidate| candidate.id == group)
        .and_then(|candidate| candidate.rack.as_ref())
}

fn require_pad(app: &app::App, group: u64, pad_note: i32) -> Result<(), String> {
    let rack = rack_of(app, group).ok_or_else(|| format!("Track group {group} is not a drum rack"))?;
    if rack.pads.iter().any(|pad| pad.pad_note == pad_note) {
        Ok(())
    } else {
        Err(format!("The rack has no pad at note {pad_note}"))
    }
}


/// Pool and library edits. `rename-rack-groove` / `delete-rack-groove` act
/// on the project pool (deleting a groove turns it off on every rack using
/// it, in one undo step); the library commands edit user `.groove` files,
/// which is not undoable.
fn apply_pool_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Result<RackGrooveEdit, String> {
    let stem = || {
        extract_string_from_payload(payload, "stem")
            .filter(|stem| !stem.trim().is_empty())
            .ok_or_else(|| format!("{name} needs a library groove stem"))
    };
    match name {
        "rename-rack-groove" => {
            let groove = groove_id(payload, name)?;
            let new_name = extract_string_from_payload(payload, "name").unwrap_or_default();
            app.rename_pool_groove_recorded(groove, &new_name)?;
        }
        "duplicate-pool-groove" => {
            let groove = groove_id(payload, name)?;
            app.duplicate_pool_groove_recorded(groove)?;
        }
        "delete-rack-groove" => {
            let groove = groove_id(payload, name)?;
            app.delete_pool_groove_recorded(groove)?;
        }
        "save-groove-to-library" => {
            let groove = groove_id(payload, name)?;
            let new_name = extract_string_from_payload(payload, "name");
            app.save_pool_groove_to_library(groove, new_name.as_deref())?;
        }
        "rename-library-groove" => {
            let new_name = extract_string_from_payload(payload, "name").unwrap_or_default();
            rename_library_groove(&stem()?, &new_name)
                .map_err(|error| format!("Could not rename the groove: {error}"))?;
        }
        "delete-library-groove" => {
            delete_library_groove(&stem()?)
                .map_err(|error| format!("Could not delete the groove: {error}"))?;
        }
        other => return Err(format!("Unknown groove command {other}")),
    }
    Ok(RackGrooveEdit::Structure)
}

/// The racks' names, in group order, joined for a sentence: "Kit A",
/// "Kit A and Kit B", "Kit A, Kit B and Kit C".
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// "Delete groove 'Take'? Kit A and Kit B play it; they will play straight."
/// The whole delete is one undo step, which the message says.
pub(crate) fn delete_pool_groove_confirm_message(groove: &str, racks: &[String]) -> String {
    let verb = if racks.len() == 1 { "plays" } else { "play" };
    let pronoun = if racks.len() == 1 { "it" } else { "they" };
    format!(
        "Delete groove '{groove}'? {} {verb} it; {pronoun} will play straight (undo restores it).",
        join_names(racks)
    )
}

/// "Delete library groove 'X'? Its file is deleted; this cannot be undone."
pub(crate) fn delete_library_groove_confirm_message(groove: &str) -> String {
    format!("Delete library groove '{groove}'? Its file is deleted; this cannot be undone.")
}

/// Commands the host confirms first (docs/rack-groove-spec.md, "Rev 2
/// UI"): deleting a pool groove some rack plays (the message lists the
/// racks) and deleting a user library file (not undoable). Returns the
/// confirm message and the Lisp payload that reruns the command with
/// `:confirmed true`; `None` runs the command at once.
pub(crate) fn confirm_before(
    name: &str,
    payload: &Value,
    app: &app::App,
) -> Option<(String, String)> {
    if extract_bool_from_payload(payload, "confirmed") {
        return None;
    }
    match name {
        "delete-rack-groove" => {
            let id = extract_usize_from_payload(payload, "groove-id")? as u64;
            let groove = sequencer::groove::pool_groove(&app.grooves, id)?;
            let racks: Vec<String> = app
                .racks_using_groove(id)
                .into_iter()
                .filter_map(|gid| app.groups.iter().find(|group| group.id == gid))
                .map(|group| group.name.clone())
                .collect();
            if racks.is_empty() {
                return None;
            }
            Some((
                delete_pool_groove_confirm_message(&groove.name, &racks),
                format!(
                    "(host-command \"delete-rack-groove\" (dict :groove-id {id} :confirmed true))"
                ),
            ))
        }
        "delete-library-groove" => {
            let stem = extract_string_from_payload(payload, "stem")?;
            let label = sequencer::groove::library::list_groove_library()
                .into_iter()
                .find(|entry| {
                    entry.tier == sequencer::groove::library::GrooveLibraryTier::User
                        && entry.stem == stem
                })
                .map_or_else(|| stem.clone(), |entry| entry.name);
            Some((
                delete_library_groove_confirm_message(&label),
                format!(
                    "(host-command \"delete-library-groove\" (dict :stem {} :confirmed true))",
                    super::file_menu::lisp_string(&stem)
                ),
            ))
        }
        _ => None,
    }
}

fn open_groove_confirm(editor: &mut Editor, message: &str, rerun: &str) {
    super::file_menu::activate_dialog_tile(editor);
    let form = format!(
        "(eseq.file-dialogs/open-confirm {} (lambda () {rerun}))",
        super::file_menu::lisp_string(message)
    );
    if let Err(error) = editor.runtime_mut().eval_str(&form) {
        editor.show_transient_message(format!("Could not ask to delete the groove: {error:?}"));
    }
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    if !COMMANDS.contains(&name) {
        return;
    }
    if let Some((message, rerun)) = confirm_before(name, &payload, app) {
        open_groove_confirm(editor, &message, &rerun);
        return;
    }
    let saved_name = (name == "save-groove-to-library")
        .then(|| {
            let id = extract_usize_from_payload(&payload, "groove-id")? as u64;
            let own = sequencer::groove::pool_groove(&app.grooves, id)?
                .name
                .clone();
            Some(
                extract_string_from_payload(&payload, "name")
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or(own),
            )
        })
        .flatten();
    let Some(result) = apply_rack_groove_command(name, &payload, app) else {
        return;
    };
    if let (Ok(_), Some(saved)) = (&result, saved_name) {
        editor.show_transient_message(format!("Saved '{saved}' to the groove library"));
    }
    match result {
        Ok(RackGrooveEdit::Amount(false)) => {}
        Ok(RackGrooveEdit::Amount(true)) => {
            // Scalar fields only: the knob being dragged is not rebuilt.
            *ctx.shared.track_groups.lock().unwrap() = app.groups.clone();
            let rt = editor.runtime_mut();
            sync_rack_groove_amount_fields(rt, &app.groups);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }
        Ok(edit) => {
            *ctx.shared.track_groups.lock().unwrap() = app.groups.clone();
            let rt = editor.runtime_mut();
            sync_groups_bindings(rt, &app.groups, &app.grooves);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            if edit == RackGrooveEdit::StructureAndPatterns {
                // Quantize source rewrote the members' patterns (delays and
                // swing): repaint the grids and the track param panels.
                ctx.shared
                    .ui_invalidations
                    .push(UiInvalidation::Pattern(PatternInvalidation::AllTracks));
                ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
            }
            ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => editor.handle_host_event(HostEvent::Status(error)),
    }
}

#[cfg(test)]
#[path = "rack_grooves_tests.rs"]
mod tests;
