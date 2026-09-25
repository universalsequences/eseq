use crate::*;

use sequencer::app::rack_grooves::RackGrooveExtractRequest;
use sequencer::groove::{
    GrooveExtractOptions, GrooveRef, GROOVE_PERIOD_ONE_BAR, GROOVE_PERIOD_TWO_BARS,
    GROOVE_RESOLUTION_SIXTEENTH, GROOVE_RESOLUTION_THIRTY_SECOND,
};

/// Rack groove commands (docs/rack-groove-spec.md, "UI"): the drum rack
/// panel's Groove section and its Extract Groove modal. Every command
/// addresses a rack by its stable `GroupId` and a groove by its id within
/// the rack.
pub(super) const COMMANDS: &[&str] = &[
    "extract-rack-groove",
    "set-rack-groove",
    "set-rack-groove-amount",
    "rename-rack-groove",
    "delete-rack-groove",
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
    let group = group_id(payload, name)?;
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
        // `key` is a picker key: `rack:<id>`, `builtin:<id>` or `off`.
        "set-rack-groove" => {
            let key = extract_string_from_payload(payload, "key").unwrap_or_default();
            let active = GrooveRef::from_picker_key(&key)?;
            let unchanged = app
                .groups
                .iter()
                .find(|candidate| candidate.id == group)
                .and_then(|candidate| candidate.rack.as_ref())
                .is_some_and(|rack| rack.groove.active == active);
            if !unchanged {
                app.set_rack_active_groove_recorded(group, active)?;
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
            let changed = app::edit::apply_rack_groove_amount_drag(app, group, |settings| {
                match amount.as_str() {
                    "timing" => settings.timing_amount = value,
                    "velocity" => settings.velocity_amount = value,
                    "random" => settings.random_amount = value,
                    _ => {}
                }
            })?;
            Ok(RackGrooveEdit::Amount(changed))
        }
        "rename-rack-groove" => {
            let groove = groove_id(payload, name)?;
            let new_name = extract_string_from_payload(payload, "name").unwrap_or_default();
            app.rename_rack_groove_recorded(group, groove, &new_name)?;
            Ok(RackGrooveEdit::Structure)
        }
        "delete-rack-groove" => {
            let groove = groove_id(payload, name)?;
            app.delete_rack_groove_recorded(group, groove)?;
            Ok(RackGrooveEdit::Structure)
        }
        other => Err(format!("Unknown rack groove command {other}")),
    }
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Some(result) = apply_rack_groove_command(name, &payload, app) else {
        return;
    };
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
            sync_groups_bindings(rt, &app.groups);
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
