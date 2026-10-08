//! One process chain edit ([`ProcessEdit`]) on one slot of one track, as
//! one recorded scene-structure entry: shared by `process-history-action`
//! (whose payload [`process_edit_from_payload`] reads) and the host kinds'
//! process setters (`edit-process`, kind-bindings spec §14.2h); on a graph
//! node's patch ([`apply_to_node_chain`]) for `edit-process` on a node's
//! process (§14.2m).

use crate::*;
use sequencer::process::{
    ParamTarget, ProcessInstanceId, ProcessLiteral, ProcessPortFanout, TrackProcessChain,
    TrackProcessSlot,
};

/// One process chain edit.
pub(super) enum ProcessEdit {
    ClearProjectLaneOverride {
        inlet: String,
    },
    SetInlet {
        inlet: String,
        literal: ProcessLiteral,
    },
    SetEnabled(bool),
    MoveSlot {
        before: Option<ProcessInstanceId>,
    },
    /// A slot the user adds: a track's roster lane, a node's slot (under
    /// the id the caller minted).
    AddRosterSlot {
        class_name: String,
    },
    RemoveSlot,
    BindPort {
        port: String,
        target: ParamTarget,
    },
    UnbindPort {
        port: String,
    },
    ClearPortBinding {
        port: String,
    },
    AddFanout {
        port: String,
        target: ParamTarget,
        lo: Option<f32>,
        hi: Option<f32>,
    },
    SetFanoutRange {
        port: String,
        index: usize,
        lo: Option<f32>,
        hi: Option<f32>,
    },
    RemoveFanout {
        port: String,
        index: usize,
    },
}

/// The [`ProcessEdit`] a `process-history-action` payload asks for.
pub(super) fn process_edit_from_payload(
    state: &SequencerState,
    op: &str,
    track: usize,
    field: &dyn Fn(&str) -> Option<Value>,
) -> Result<ProcessEdit, String> {
    let string = |name: &str, missing: &str| {
        field(name)
            .and_then(|value| match value {
                Value::String(value) => Some(value),
                _ => None,
            })
            .ok_or_else(|| missing.to_string())
    };
    let number = |name: &str| {
        field(name).and_then(|value| match value {
            Value::Number(value) => Some(value as f32),
            _ => None,
        })
    };
    let index = || {
        field("index")
            .and_then(|value| match value {
                Value::Number(value) if value >= 0.0 => Some(value as usize),
                _ => None,
            })
            .ok_or_else(|| "Process fan-out index is missing".to_string())
    };
    let port = || string("port", "Process port is missing");
    let target = |missing: &str| {
        let target = field("target").ok_or_else(|| missing.to_string())?;
        natives::param_target_from_value(state, track, &target)
    };
    Ok(match op {
        "clear-project-lane-override" => ProcessEdit::ClearProjectLaneOverride {
            inlet: string("inlet", "Process lane inlet is missing")?,
        },
        "set-inlet" => {
            let inlet = string("inlet", "Process inlet is missing")?;
            let literal =
                match field("value").ok_or_else(|| "Process inlet value is missing".to_string())? {
                    Value::Number(value) => ProcessLiteral::Number(value),
                    Value::Bool(value) => ProcessLiteral::Bool(value),
                    Value::String(value) => ProcessLiteral::String(value),
                    Value::Keyword(value) => ProcessLiteral::Keyword(value),
                    Value::Symbol(value) => ProcessLiteral::Symbol(value),
                    Value::Nil => ProcessLiteral::Nil,
                    _ => return Err("Unsupported process inlet literal".to_string()),
                };
            ProcessEdit::SetInlet { inlet, literal }
        }
        "set-enabled" => ProcessEdit::SetEnabled(
            field("enabled")
                .and_then(|value| match value {
                    Value::Bool(value) => Some(value),
                    _ => None,
                })
                .ok_or_else(|| "Process enabled state is missing".to_string())?,
        ),
        "move-slot" => ProcessEdit::MoveSlot {
            before: match field("before-instance-id") {
                Some(Value::Number(value)) if value >= 0.0 => Some(ProcessInstanceId(value as u64)),
                Some(Value::Nil) | None => None,
                _ => return Err("Process move target is invalid".to_string()),
            },
        },
        "add-roster-slot" => ProcessEdit::AddRosterSlot {
            class_name: string("class-name", "Process class name is missing")?,
        },
        "remove-slot" => ProcessEdit::RemoveSlot,
        "bind-port" => ProcessEdit::BindPort {
            port: port()?,
            target: target("Process binding target is missing")?,
        },
        "unbind-port" => ProcessEdit::UnbindPort { port: port()? },
        "clear-port-binding" => ProcessEdit::ClearPortBinding { port: port()? },
        "add-fanout" => ProcessEdit::AddFanout {
            port: port()?,
            target: target("Process fan-out target is missing")?,
            lo: number("lo"),
            hi: number("hi"),
        },
        "set-fanout-range" => ProcessEdit::SetFanoutRange {
            port: port()?,
            index: index()?,
            lo: number("lo"),
            hi: number("hi"),
        },
        "remove-fanout" => ProcessEdit::RemoveFanout {
            port: port()?,
            index: index()?,
        },
        _ => return Err(format!("Unknown process history operation {op}")),
    })
}

/// Apply `edit` to the slot `instance_id` of `track` as one recorded scene
/// structure edit (undo restores it): `all_tracks` writes the shared
/// project slot (every track), else a project slot forks for this track.
/// Returns whether it changed the model: a missing target or slot, or an
/// edit that changes nothing, is `Ok(false)` and records nothing (a shared
/// edit reports a change only when one happened; a per-track one whenever
/// its slot matched). The caller queues the `UiInvalidation::ProcessChain`
/// refresh.
pub(super) fn apply_process_edit(
    app: &mut app::App,
    track: usize,
    instance_id: ProcessInstanceId,
    all_tracks: bool,
    edit: ProcessEdit,
) -> Result<bool, String> {
    // A bus-send target needs a persistent graph edge on every track
    // it will write to, exactly like a send p-lock at a zero baseline:
    // the scheduler addresses sends through runtime targets that only
    // exist once the track lists that destination. Done before the
    // recorded mutation so the graph edit keeps its own history entry.
    if let ProcessEdit::BindPort {
        target: ParamTarget::BusSend { bus },
        ..
    }
    | ProcessEdit::AddFanout {
        target: ParamTarget::BusSend { bus },
        ..
    } = &edit
    {
        add_send_edges(app, track, all_tracks, sequencer::sequencer::BusId(*bus));
    }
    // An unchanged model rolls the recorded mutation back (it records an
    // entry for any `Ok`); this says the error was that, not a failure.
    let mut unchanged = false;
    let result = app.apply_recorded_scene_structure_mutation("Edit process chain", |app| {
        if apply_to_state(&app.state, track, instance_id, all_tracks, edit) {
            Ok(())
        } else {
            unchanged = true;
            Err(String::new())
        }
    });
    match result {
        Ok(()) => Ok(true),
        Err(_) if unchanged => Ok(false),
        Err(error) => Err(error),
    }
}

/// `edit` on slot `instance_id` of a graph node's `chain` (as the removed
/// `graph-node-process-*` wiring natives did): the chain is the node's own (no
/// project layer to fork, no roster), so every edit writes the slot itself.
/// Returns whether the chain changed; a slot, fan-out entry or move target
/// that is gone is an error.
pub(super) fn apply_to_node_chain(
    chain: &mut TrackProcessChain,
    instance_id: ProcessInstanceId,
    edit: ProcessEdit,
) -> Result<bool, String> {
    fn slot_of(
        chain: &mut TrackProcessChain,
        id: ProcessInstanceId,
    ) -> Result<&mut TrackProcessSlot, String> {
        (chain.slots.iter_mut())
            .find(|slot| slot.instance_id == id)
            .ok_or_else(|| "the process is gone".to_string())
    }
    fn fanout_of<'a>(
        slot: &'a mut TrackProcessSlot,
        port: &str,
    ) -> Result<&'a mut Vec<ProcessPortFanout>, String> {
        (slot.fanout.get_mut(port)).ok_or_else(|| "the fan-out entry is gone".to_string())
    }
    let changed = match edit {
        ProcessEdit::ClearProjectLaneOverride { .. } => {
            return Err("a node's process has no project lane".to_string());
        }
        ProcessEdit::AddRosterSlot { class_name } => {
            chain
                .slots
                .push(TrackProcessSlot::new(instance_id, class_name));
            true
        }
        ProcessEdit::RemoveSlot => {
            if !chain.remove_slot_and_wires(instance_id) {
                return Err("the process is gone".to_string());
            }
            true
        }
        ProcessEdit::MoveSlot { before } => {
            let at = |chain: &TrackProcessChain, id| {
                (chain.slots.iter()).position(|slot| slot.instance_id == id)
            };
            let from = at(chain, instance_id).ok_or("the process is gone")?;
            if before.is_some_and(|before| at(chain, before).is_none()) {
                return Err("the process to move before is gone".to_string());
            }
            let slot = chain.slots.remove(from);
            let to = before.and_then(|before| at(chain, before));
            let to = to.unwrap_or(chain.slots.len());
            chain.slots.insert(to, slot);
            from != to
        }
        ProcessEdit::SetInlet { inlet, literal } => {
            let slot = slot_of(chain, instance_id)?;
            slot.inlets.insert(inlet, literal.clone()) != Some(literal)
        }
        ProcessEdit::SetEnabled(enabled) => {
            let slot = slot_of(chain, instance_id)?;
            std::mem::replace(&mut slot.enabled, enabled) != enabled
        }
        ProcessEdit::BindPort { port, target } => {
            let slot = slot_of(chain, instance_id)?;
            let reconnected = slot.unbound_ports.remove(&port);
            let bound = slot.bindings.insert(port, Some(target.clone()));
            reconnected || bound != Some(Some(target))
        }
        ProcessEdit::UnbindPort { port } => {
            let slot = slot_of(chain, instance_id)?;
            let had_binding = slot.bindings.remove(&port).is_some();
            slot.unbound_ports.insert(port) | had_binding
        }
        ProcessEdit::ClearPortBinding { port } => {
            let slot = slot_of(chain, instance_id)?;
            slot.bindings.remove(&port).is_some() | slot.unbound_ports.remove(&port)
        }
        ProcessEdit::AddFanout {
            port,
            target,
            lo,
            hi,
        } => {
            let slot = slot_of(chain, instance_id)?;
            // At the slot's own output range: identity scaling until the
            // user narrows it.
            let (low, high) = sequencer::process::process_slot_output_range(slot);
            let entry = ProcessPortFanout {
                target,
                lo: lo.unwrap_or(low),
                hi: hi.unwrap_or(high),
            };
            slot.fanout.entry(port).or_default().push(entry);
            true
        }
        ProcessEdit::SetFanoutRange {
            port,
            index,
            lo,
            hi,
        } => {
            let list = fanout_of(slot_of(chain, instance_id)?, &port)?;
            let entry = list
                .get_mut(index)
                .ok_or_else(|| "the fan-out entry is gone".to_string())?;
            let was = (entry.lo, entry.hi);
            entry.lo = lo.unwrap_or(entry.lo);
            entry.hi = hi.unwrap_or(entry.hi);
            was != (entry.lo, entry.hi)
        }
        ProcessEdit::RemoveFanout { port, index } => {
            let slot = slot_of(chain, instance_id)?;
            let list = fanout_of(slot, &port)?;
            if index >= list.len() {
                return Err("the fan-out entry is gone".to_string());
            }
            list.remove(index);
            if list.is_empty() {
                slot.fanout.remove(&port);
            }
            true
        }
    };
    Ok(changed)
}

/// Give `track` (or every track) a zero send to `destination` where it has
/// none (see [`apply_process_edit`]).
fn add_send_edges(
    app: &mut app::App,
    track: usize,
    all_tracks: bool,
    destination: sequencer::sequencer::BusId,
) {
    let tracks: Vec<usize> = if all_tracks {
        (0..app.state.active_track_count()).collect()
    } else {
        vec![track]
    };
    for track in tracks {
        let Some(params) = app.state.pattern.track_params.get(track) else {
            continue;
        };
        let mut sends = params.sends();
        if sends.iter().any(|send| send.destination == destination) {
            continue;
        }
        sends.push(sequencer::sequencer::TrackSendSnapshot {
            destination,
            amount: 0.0,
        });
        app::apply_command(app, app::AppCommand::SetTrackSends { track, sends });
    }
}

/// The state edit behind [`apply_process_edit`]; whether it applied.
fn apply_to_state(
    state: &SequencerState,
    track: usize,
    instance_id: ProcessInstanceId,
    all_tracks: bool,
    edit: ProcessEdit,
) -> bool {
    let fanout = |port: &str, edit: &mut dyn FnMut(&mut Vec<ProcessPortFanout>)| {
        state.edit_process_port_fanout(track, instance_id, port, all_tracks, edit)
    };
    match edit {
        ProcessEdit::ClearProjectLaneOverride { inlet } => {
            state.clear_project_process_lane_override(track, instance_id, &inlet)
        }
        ProcessEdit::SetInlet { inlet, literal } if all_tracks => {
            state.set_process_inlet_value(instance_id, &inlet, literal)
        }
        ProcessEdit::SetInlet { inlet, literal } => {
            state.set_track_process_inlet_value(track, instance_id, &inlet, literal)
        }
        ProcessEdit::SetEnabled(enabled) if all_tracks => {
            state.set_process_slot_enabled_all(instance_id, enabled)
        }
        ProcessEdit::SetEnabled(enabled) => {
            state.set_track_process_slot_enabled(track, instance_id, enabled)
        }
        ProcessEdit::MoveSlot { before } => {
            state.move_track_process_slot_before(track, instance_id, before)
        }
        // The id was minted by the caller; the state layer keeps it
        // unless another slot took it first.
        ProcessEdit::AddRosterSlot { class_name } => state
            .add_track_roster_slot_with_id(track, &class_name, Some(instance_id))
            .is_some(),
        // A roster slot exists in every scene, so removing it goes
        // through the roster (eseq-53y7); project-layer slots and
        // script-authored track slots keep the per-pattern detach.
        ProcessEdit::RemoveSlot => {
            if sequencer::process::is_track_roster_instance_id(instance_id) {
                state.remove_track_roster_slot(track, instance_id)
            } else {
                state.remove_track_process_slot(track, instance_id)
            }
        }
        ProcessEdit::BindPort { port, target } if all_tracks => {
            state.set_process_port_binding_for_instance(instance_id, &port, target)
        }
        ProcessEdit::BindPort { port, target } => {
            state.set_process_port_binding(track, instance_id, &port, target)
        }
        ProcessEdit::UnbindPort { port } if all_tracks => {
            state.unbind_process_port_for_instance(instance_id, &port)
        }
        ProcessEdit::UnbindPort { port } => state.unbind_process_port(track, instance_id, &port),
        ProcessEdit::ClearPortBinding { port } if all_tracks => {
            state.clear_process_port_binding_for_instance(instance_id, &port)
        }
        ProcessEdit::ClearPortBinding { port } => {
            state.clear_process_port_binding(track, instance_id, &port)
        }
        ProcessEdit::AddFanout {
            port,
            target,
            lo,
            hi,
        } => {
            // Default to the slot's own output range: identity
            // scaling until the user narrows it.
            let source = state
                .composed_track_process_chain(track)
                .and_then(|chain| {
                    (chain.slots.into_iter()).find(|slot| slot.instance_id == instance_id)
                })
                .map(|slot| sequencer::process::process_slot_output_range(&slot))
                .unwrap_or((0.0, 1.0));
            let mut entry = Some(ProcessPortFanout {
                target,
                lo: lo.unwrap_or(source.0),
                hi: hi.unwrap_or(source.1),
            });
            fanout(&port, &mut |list| list.extend(entry.take()))
        }
        ProcessEdit::SetFanoutRange {
            port,
            index,
            lo,
            hi,
        } => fanout(&port, &mut |list| {
            if let Some(entry) = list.get_mut(index) {
                if let Some(lo) = lo {
                    entry.lo = lo;
                }
                if let Some(hi) = hi {
                    entry.hi = hi;
                }
            }
        }),
        ProcessEdit::RemoveFanout { port, index } => fanout(&port, &mut |list| {
            if index < list.len() {
                list.remove(index);
            }
        }),
    }
}
