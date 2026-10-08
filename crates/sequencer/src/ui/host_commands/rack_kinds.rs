//! The drum rack kinds' setters (kind-bindings spec §14, stage 7h):
//! `set-pad` (`pad.note`, `choke`, `role`), `set-rack-clip`
//! (`rack-clip.name`, `own-groove`), `set-groove` (`groove.pool-groove`,
//! `enabled`, `timing`, `velocity`, `random`, `scale`; with `:track-id` a
//! pad share's `pad-amount` and `pad-enabled`) and `set-pool-groove`
//! (`pool-groove.name`).
//!
//! A rack is named by its stable group id, a pad by its member track's
//! stable `TrackId`, a clip by its stable clip id (0 for a rack's own
//! groove) and a pool groove by its id, all resolved when the command
//! lands, so a pad move, a reorder or a launch in between cannot retarget
//! them; a gone rack, pad, clip or groove is an error. Setters act only
//! where the model differs and go through the drum rack panel's recorded
//! edits (one bus/group structure entry each, which undo restores), landing
//! like them (`sync_rack_pad_map`, [`super::rack_grooves::groove_edit_landed`]).
//! Values follow the value rule ([`SetValue`]): an `:int` field takes an
//! integer in range, a number a finite number in range, a flag a bool, a
//! role one of `pad-role-options` (case-insensitive) or "" for Standard, a
//! name a non-empty string; anything else is an error that changes nothing.
//! Gestures as [`super::ScriptEdit`]: a groove amount or pad share set while
//! the pointer is down joins the script's drag (one entry, as a knob drag:
//! `apply_rack_groove_amount_drag`); anything else is its own entry.
//! `set-groove` on a clip that follows the rack's groove gives it its own (a
//! copy of the rack's, `groove_target_mut`) in that same entry, landing as
//! a structure edit (the clip gains a groove instance). Capture applies the
//! setters too ([`apply_command`]).

use super::rack_grooves::{groove_edit_landed, Amount, RackGrooveEdit};
use super::track_settings::{command_track, SetValue};
use crate::*;
use sequencer::groove::{
    pool_groove, GROOVE_PAD_AMOUNT_MAX, GROOVE_RANDOM_AMOUNT_MAX, GROOVE_SCALES,
    GROOVE_TIMING_AMOUNT_MAX, GROOVE_VELOCITY_AMOUNT_MAX,
};
use sequencer::project::{PadRole, ProjectRackConfig, ProjectTrackGroup};
use sequencer::sequencer::{DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE};
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &["set-pad", "set-rack-clip", "set-groove", "set-pool-groove"];

/// The highest choke group a pad takes (the panel offers 1-16).
pub(crate) const CHOKE_GROUPS: usize = 16;

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// One edit a setter asks for, resolved against the model when the command
/// lands.
enum RackEdit {
    PadNote {
        group: u64,
        from: i32,
        to: i32,
    },
    PadChoke {
        group: u64,
        note: i32,
        choke: Option<u8>,
    },
    PadRole {
        group: u64,
        note: i32,
        role: Option<PadRole>,
    },
    ClipName {
        group: u64,
        clip: u64,
        name: String,
    },
    OwnGroove {
        group: u64,
        clip: u64,
        own: bool,
    },
    PoolGroove {
        group: u64,
        clip: Option<u64>,
        groove: Option<u64>,
    },
    Enabled {
        group: u64,
        clip: Option<u64>,
        enabled: bool,
    },
    Scale {
        group: u64,
        clip: Option<u64>,
        scale: f32,
    },
    Amount {
        group: u64,
        clip: Option<u64>,
        amount: Amount,
        value: f32,
        /// The clip follows the rack's groove: the edit gives it its own.
        forks: bool,
    },
    PadEnabled {
        group: u64,
        clip: Option<u64>,
        note: i32,
        enabled: bool,
    },
    PoolName {
        groove: u64,
        name: String,
    },
}

impl RackEdit {
    /// A value a drag moves: joins the script's drag while the pointer is
    /// down.
    fn continuous(&self) -> bool {
        matches!(self, Self::Amount { .. })
    }

    /// How the edit lands: the pad map's republish, or a groove edit's (an
    /// amount that gives a clip its own groove adds a groove instance, so
    /// it lands as a structure edit).
    fn landing(&self) -> Option<RackGrooveEdit> {
        match self {
            Self::PadNote { .. }
            | Self::PadChoke { .. }
            | Self::PadRole { .. }
            | Self::ClipName { .. } => None,
            Self::Amount { forks: false, .. } => Some(RackGrooveEdit::Amount(true)),
            _ => Some(RackGrooveEdit::Structure),
        }
    }

    /// Apply through the panel's recorded edits (`drags`: an amount joins
    /// the script's drag). Returns whether the model changed.
    fn apply(self, app: &mut app::App, drags: bool) -> Result<bool, String> {
        match self {
            Self::PadNote { group, from, to } => app.set_rack_pad_note_recorded(group, from, to),
            Self::PadChoke { group, note, choke } => {
                app.set_rack_pad_choke_group_recorded(group, note, choke)
            }
            Self::PadRole { group, note, role } => {
                app.set_rack_pad_role_recorded(group, note, role)
            }
            Self::ClipName { group, clip, name } => {
                app.rename_rack_clip_recorded(group, clip, &name)
            }
            Self::OwnGroove { group, clip, own } => {
                app.set_rack_clip_own_groove_recorded(group, clip, own)
            }
            Self::PoolGroove {
                group,
                clip,
                groove,
            } => app.set_rack_active_groove_recorded(group, clip, groove),
            Self::Enabled {
                group,
                clip,
                enabled,
            } => app.set_rack_groove_enabled_recorded(group, clip, enabled),
            Self::Scale { group, clip, scale } => {
                app.set_rack_groove_scale_recorded(group, clip, scale)
            }
            Self::PadEnabled {
                group,
                clip,
                note,
                enabled,
            } => app.set_rack_groove_pad_enabled_recorded(group, clip, note, enabled),
            Self::PoolName { groove, name } => app.rename_pool_groove_recorded(groove, &name),
            Self::Amount {
                group,
                clip,
                amount,
                value,
                ..
            } => {
                let mutate = |settings: &mut _| amount.set(settings, value);
                return if drags {
                    app::edit::apply_rack_groove_amount_drag(app, group, clip, mutate)
                } else {
                    app.set_rack_groove_amounts_recorded(group, clip, mutate)
                };
            }
        }
        .map(|()| true)
    }
}

/// The rack `:group-id` names now.
fn command_rack<'a>(
    app: &'a app::App,
    map: &Payload,
) -> Result<(u64, &'a ProjectTrackGroup, &'a ProjectRackConfig), String> {
    let id = SetValue::of(map, "group-id", "group-id").id("a group id")?;
    let group = (app.groups.iter())
        .find(|group| group.id == id)
        .ok_or("the rack is gone")?;
    let rack = group.rack.as_ref().ok_or("the group is no drum rack")?;
    Ok((id, group, rack))
}

/// The pad (its index in the pad map) the member track `:track-id` names
/// backs now.
fn command_pad(app: &app::App, group: &ProjectTrackGroup, map: &Payload) -> Result<usize, String> {
    let track = command_track(app, map)?;
    group
        .rack_pad_index_of_track(track)
        .ok_or_else(|| "the pad is gone".to_string())
}

/// What `set-pad` asks for; `Ok(None)` when the pad already is so.
fn pad_request(app: &app::App, map: &Payload) -> Result<Option<RackEdit>, String> {
    let (group, rack_group, rack) = command_rack(app, map)?;
    let index = command_pad(app, rack_group, map)?;
    let pad = &rack.pads[index];
    let note = pad.pad_note;
    let (field, value) = SetValue::field(map)?;
    match field.as_str() {
        "note" => {
            let range = (
                DRUM_RACK_FIRST_PAD_NOTE.into(),
                DRUM_RACK_LAST_PAD_NOTE.into(),
            );
            let to = value.signed(range.0, range.1)? as i32;
            Ok((to != note).then_some(RackEdit::PadNote {
                group,
                from: note,
                to,
            }))
        }
        "choke" => {
            let choke = value.integer(0, CHOKE_GROUPS)?;
            let choke = (choke > 0).then_some(choke as u8);
            let changed = choke != rack.choke_group(index);
            Ok(changed.then_some(RackEdit::PadChoke { group, note, choke }))
        }
        "role" => {
            let role = match value.label()? {
                "" => None,
                key => Some(
                    PadRole::from_key_ignore_case(key)
                        .map_or_else(|| value.fail("one of pad-role-options, or \"\""), Ok)?,
                ),
            };
            let changed = role != pad.role;
            Ok(changed.then_some(RackEdit::PadRole { group, note, role }))
        }
        other => Err(format!("unknown pad field '{other}'")),
    }
}

/// The current name of rack `group`'s clip `clip`; an error when it is
/// gone.
fn require_clip(app: &app::App, group: u64, clip: u64) -> Result<String, String> {
    app.state
        .with_scenes(|scenes| {
            let bank = scenes.rack_bank(group)?;
            Some(bank.clip(clip)?.name.clone())
        })
        .ok_or_else(|| "the clip is gone".to_string())
}

/// What `set-rack-clip` asks for; `Ok(None)` when the clip already is so.
fn rack_clip_request(app: &app::App, map: &Payload) -> Result<Option<RackEdit>, String> {
    let (group, _, rack) = command_rack(app, map)?;
    let clip = SetValue::of(map, "clip-id", "clip-id").id("a clip id")?;
    let current = require_clip(app, group, clip)?;
    let (field, value) = SetValue::field(map)?;
    match field.as_str() {
        "name" => {
            let name = value.name()?;
            let changed = name != current;
            let name = name.to_string();
            Ok(changed.then_some(RackEdit::ClipName { group, clip, name }))
        }
        "own-groove" => {
            let own = value.flag()?;
            let changed = own != rack.clip_groove(clip).is_some();
            Ok(changed.then_some(RackEdit::OwnGroove { group, clip, own }))
        }
        other => Err(format!("unknown rack clip field '{other}'")),
    }
}

/// What `set-groove` asks for; `Ok(None)` when the groove already is so.
fn groove_request(app: &app::App, map: &Payload) -> Result<Option<RackEdit>, String> {
    let (group, rack_group, rack) = command_rack(app, map)?;
    // 0 is the rack's own groove. A clip is compared with the groove it
    // plays; one that follows the rack's gets its own (a copy) as the edit
    // applies (`groove_target_mut`), in the edit's entry.
    let clip = SetValue::of(map, "clip-id", "clip-id").id("a clip id")?;
    let clip = (clip != 0).then_some(clip);
    if let Some(clip) = clip {
        require_clip(app, group, clip)?;
    }
    let settings = rack.groove_for_clip(clip);
    let forks = clip.is_some_and(|clip| rack.clip_groove(clip).is_none());
    let (field, value) = SetValue::field(map)?;
    let amount = |amount: Amount, current: f32, max: f32| -> Result<Option<RackEdit>, String> {
        let value = value.number(0.0, max.into())? as f32;
        Ok((value != current).then_some(RackEdit::Amount {
            group,
            clip,
            amount,
            value,
            forks,
        }))
    };
    match field.as_str() {
        // The pool groove's id (the kind passes `pg.groove-id`), or nil.
        "pool-groove" => {
            let groove = value.id_or_nil("a pool groove or nil")?;
            if groove.is_some_and(|id| pool_groove(&app.grooves, id).is_none()) {
                return value.fail("a groove of the pool");
            }
            let changed = groove != settings.active;
            Ok(changed.then_some(RackEdit::PoolGroove {
                group,
                clip,
                groove,
            }))
        }
        "enabled" => {
            let enabled = value.flag()?;
            let changed = enabled != settings.enabled;
            Ok(changed.then_some(RackEdit::Enabled {
                group,
                clip,
                enabled,
            }))
        }
        "scale" => {
            let scale = value.number(0.0, 2.0)? as f32;
            if !GROOVE_SCALES.contains(&scale) {
                return value.fail("one of groove-scale-options");
            }
            let changed = scale != settings.scale;
            Ok(changed.then_some(RackEdit::Scale { group, clip, scale }))
        }
        "timing" => amount(
            Amount::Timing,
            settings.timing_amount,
            GROOVE_TIMING_AMOUNT_MAX,
        ),
        "velocity" => amount(
            Amount::Velocity,
            settings.velocity_amount,
            GROOVE_VELOCITY_AMOUNT_MAX,
        ),
        "random" => amount(
            Amount::Random,
            settings.random_amount,
            GROOVE_RANDOM_AMOUNT_MAX,
        ),
        "pad-amount" | "pad-enabled" => {
            let note = rack.pads[command_pad(app, rack_group, map)?].pad_note;
            let share = settings.pad(note);
            if field == "pad-amount" {
                return amount(Amount::Pad(note), share.amount, GROOVE_PAD_AMOUNT_MAX);
            }
            let enabled = value.flag()?;
            Ok((enabled != share.enabled).then_some(RackEdit::PadEnabled {
                group,
                clip,
                note,
                enabled,
            }))
        }
        other => Err(format!("unknown groove field '{other}'")),
    }
}

/// What `set-pool-groove` asks for; `Ok(None)` when the groove already is
/// so.
fn pool_groove_request(app: &app::App, map: &Payload) -> Result<Option<RackEdit>, String> {
    let groove = SetValue::of(map, "groove-id", "groove-id").id("a groove id")?;
    let current = pool_groove(&app.grooves, groove).ok_or("the groove is gone")?;
    let (field, value) = SetValue::field(map)?;
    match field.as_str() {
        "name" => {
            let name = value.name()?;
            let changed = name != current.name;
            let name = name.to_string();
            Ok(changed.then_some(RackEdit::PoolName { groove, name }))
        }
        other => Err(format!("unknown pool groove field '{other}'")),
    }
}

/// Apply `edit` as a script edit and land it.
fn apply_edit(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    edit: RackEdit,
) -> Result<(), String> {
    let continuous = edit.continuous();
    let landing = edit.landing();
    let script = super::ScriptEdit::begin(app, ctx, continuous);
    let drags = script.drags(ctx);
    let outcome = script.apply_with(app, |app| edit.apply(app, drags));
    let changed = matches!(outcome, Ok(true));
    script.end(app, ctx, changed);
    if changed {
        match landing {
            None => super::drum_rack_v2::sync_rack_pad_map(
                app,
                editor,
                &ctx.shared.track_groups,
                &ctx.shared.ui_epoch,
            ),
            Some(edit) => groove_edit_landed(app, editor, ctx, edit),
        }
    }
    outcome.map(|_| ())
}

/// What setter `name` asks for (`Ok(None)` when the model already is so,
/// or for a command that is not a setter's).
fn request(name: &str, payload: &Value, app: &app::App) -> Result<Option<RackEdit>, String> {
    let Value::Map(map) = payload else {
        return Err("the payload is not a dict".to_string());
    };
    match name {
        "set-pad" => pad_request(app, map),
        "set-rack-clip" => rack_clip_request(app, map),
        "set-groove" => groove_request(app, map),
        "set-pool-groove" => pool_groove_request(app, map),
        _ => Ok(None),
    }
}

/// Apply setter `name` to the model as its recorded edit, unlanded: the
/// capture harness's route, so a fixture lays a rack out with the kind
/// setters. `None` for a command that is not one of [`COMMANDS`].
pub(crate) fn apply_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Option<Result<(), String>> {
    COMMANDS
        .contains(&name)
        .then(|| match request(name, payload, app)? {
            Some(edit) => edit.apply(app, false).map(|_| ()),
            None => Ok(()),
        })
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = request(name, &payload, app).and_then(|edit| match edit {
        Some(edit) => apply_edit(app, editor, ctx, edit),
        None => Ok(()),
    });
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
