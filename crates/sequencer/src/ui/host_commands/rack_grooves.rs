use crate::*;

use sequencer::app::rack_grooves::RackGrooveExtractRequest;
use sequencer::groove::library::{
    load_library_groove,
};
use sequencer::groove::{
    GrooveChoice, GrooveExtractOptions, RackGrooveSettings, GROOVE_PERIOD_ONE_BAR,
    GROOVE_PERIOD_TWO_BARS, GROOVE_RESOLUTION_SIXTEENTH, GROOVE_RESOLUTION_THIRTY_SECOND,
};

/// Groove commands (docs/rack-groove-spec.md, "UI"): the drum rack panel's
/// Groove section, its Extract Groove modal, and the pool/library edits the
/// Grooves tab shares. Rack commands address a rack by its stable `GroupId`;
/// a groove is a project POOL id (`groove-id`), a library file a
/// `stem` in the user tier.
pub(super) const COMMANDS: &[&str] = &[
    "extract-rack-groove",
    "set-rack-groove",
    "apply-rack-groove-to-all-clips",
    "duplicate-pool-groove",
    "delete-rack-groove",
    "save-groove-to-library",
];

/// Commands that act on the pool or the library, not on one rack: no
/// `group-id` needed.
const POOL_COMMANDS: &[&str] = &[
    "duplicate-pool-groove",
    "delete-rack-groove",
    "save-groove-to-library",
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

/// Which groove amount an amount edit sets: the rack's timing, velocity or
/// random amount, or one pad's share (by its note): the kinds'
/// `set-groove`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Amount {
    Timing,
    Velocity,
    Random,
    Pad(i32),
}

impl Amount {
    /// "timing", "velocity" or "random".
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "timing" => Some(Self::Timing),
            "velocity" => Some(Self::Velocity),
            "random" => Some(Self::Random),
            _ => None,
        }
    }

    pub(crate) fn set(self, settings: &mut RackGrooveSettings, value: f32) {
        match self {
            Self::Timing => settings.timing_amount = value,
            Self::Velocity => settings.velocity_amount = value,
            Self::Random => settings.random_amount = value,
            Self::Pad(note) => settings.pad_mut(note).amount = value,
        }
    }
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
    // The clip whose groove the edit targets (`clip-id`; absent, 0 or -1 for
    // the rack's own, clip ids starting at 1): the buffer names the clip it
    // shows, so a launch landing between drawing and clicking cannot
    // redirect the edit.
    let clip = extract_i32_from_payload(payload, "clip-id")
        .filter(|clip| *clip > 0)
        .map(|clip| clip as u64);
    // A clip deleted between drawing and clicking: editing it would mint a
    // `clip_grooves` entry for a clip that no longer exists.
    if let Some(clip) = clip {
        let exists = app.state.with_scenes(|scenes| {
            scenes
                .rack_bank(group)
                .is_some_and(|bank| bank.clip(clip).is_some())
        });
        if !exists {
            return Err("That rack clip no longer exists".to_string());
        }
    }
    let target = |app: &app::App| {
        rack_of(app, group).map(|rack| rack.groove_for_clip(clip).clone())
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


/// Pool and library edits. `duplicate-pool-groove` / `delete-rack-groove`
/// act on the project pool (deleting a groove turns it off on every rack
/// using it, in one undo step); `save-groove-to-library` writes a user
/// `.groove` file. (The legacy rename / amount / pad / library-file
/// commands went in eseq-0l17.81: the kinds' `set-groove` and
/// `set-pool-groove` replace them.)
fn apply_pool_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Result<RackGrooveEdit, String> {
    match name {
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

/// Commands the host confirms first (docs/rack-groove-spec.md, "Rev 2
/// UI"): deleting a pool groove some rack plays (the message lists the
/// racks). Returns the
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
        Ok(edit) => groove_edit_landed(app, editor, ctx, edit),
        Err(error) => editor.handle_host_event(HostEvent::Status(error)),
    }
}

/// Land an applied groove edit (shared with the kinds' setters, `set-groove`
/// and `set-rack-clip`): an edit that changed something shares the groups,
/// which the host kinds publish on their next sync. An amount does nothing
/// more, so the knob being dragged is not rebuilt; anything else also runs a
/// reactive cycle and bumps the UI epoch (and, after a quantizing extract,
/// repaints the members' patterns).
pub(super) fn groove_edit_landed(
    app: &app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    edit: RackGrooveEdit,
) {
    if edit == RackGrooveEdit::Amount(false) {
        return;
    }
    ctx.shared
        .track_groups
        .lock()
        .unwrap()
        .clone_from(&app.groups);
    // Already shared: the next tick's groups reconcile has nothing to pull.
    ctx.frame.prev_groups.clone_from(&app.groups);
    match edit {
        RackGrooveEdit::Amount(_) => {}
        edit => {
            editor.runtime_mut().run_reactive_cycle();
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
    }
}

#[cfg(test)]
#[path = "rack_grooves_tests.rs"]
mod tests;
