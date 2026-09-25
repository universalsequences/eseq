//! Rack grooves on the drum rack panel (docs/rack-groove-spec.md, "UI").
//!
//! Two kinds of fields, split so a knob drag never rebuilds the section it
//! is dragging in:
//!
//! - `SEQ.rack-grooves` is STRUCTURAL: one entry per drum rack with its
//!   groove list, the picker (labels + parallel keys), the active groove and
//!   the heatmap of the active groove. It changes on extract / pick /
//!   rename / delete / pad-map edits.
//! - `SEQ.rack-groove-{timing,velocity,random}-<group id>` are the amounts,
//!   scalar fields the Timing / Velocity / Random knobs bind to.
//!
//! `SEQ.groove-builtins` lists the generic grooves (`{key name}`), which
//! never change at runtime.

use super::*;
use sequencer::groove::{builtin_grooves, GrooveRef, ProjectGroove, GROOVE_PERIOD_ONE_BAR};
use sequencer::project::{ProjectRackConfig, ProjectTrackGroup};

/// Picker label for "no groove".
pub(crate) const GROOVE_PICKER_OFF: &str = "Off";

pub(crate) fn rack_groove_amount_field(amount: &str, group_id: u64) -> String {
    format!("rack-groove-{amount}-{group_id}")
}

/// Rack grooves first (This rack), then the generic built-ins, then Off.
/// Labels are made unique (a duplicate rack groove name gets its id), so the
/// dropdown's label round-trips to exactly one key.
fn picker(rack: &ProjectRackConfig) -> (Vec<String>, Vec<String>) {
    let mut labels: Vec<String> = Vec::new();
    let mut keys = Vec::new();
    for groove in &rack.grooves {
        let mut label = groove.name.clone();
        if label.is_empty() || labels.contains(&label) {
            label = format!("{} #{}", groove.name, groove.id);
        }
        labels.push(label);
        keys.push(GrooveRef::Rack(groove.id).picker_key());
    }
    for builtin in builtin_grooves() {
        let mut label = builtin.groove.name.clone();
        if labels.contains(&label) {
            label = format!("{label} (generic)");
        }
        labels.push(label);
        keys.push(GrooveRef::Builtin(builtin.id.clone()).picker_key());
    }
    labels.push(GROOVE_PICKER_OFF.to_string());
    keys.push("off".to_string());
    (labels, keys)
}

/// How many times the groove's period repeats across the heatmap: a groove
/// shorter than a bar (the two-slot MPC swings) is tiled out to one bar, so
/// the map always reads as a bar of the pocket.
fn heat_repeats(groove: &ProjectGroove) -> usize {
    if groove.period_beats >= GROOVE_PERIOD_ONE_BAR - 1e-9 || groove.period_beats <= 0.0 {
        1
    } else {
        ((GROOVE_PERIOD_ONE_BAR / groove.period_beats).round() as usize).max(1)
    }
}

/// One heatmap row: label, whether the row is the pad's own (measured) row
/// or the shared fallback, per-cell offsets in slots (negative = early) and
/// per-cell `measured` flags (filled/guessed cells are drawn dimmed).
fn heat_row(label: String, pad_note: Option<i32>, own: bool, groove: &ProjectGroove) -> Value {
    let row = match pad_note {
        Some(note) => groove.row_for_pad(note),
        None => &groove.shared_row,
    };
    let repeats = heat_repeats(groove);
    let cells = (0..repeats).flat_map(|_| row.slots.iter());
    map_value([
        ("label", Value::String(label.into())),
        (
            "pad-note",
            pad_note.map_or(Value::Nil, |note| Value::Number(note as f64)),
        ),
        ("own", Value::Bool(own)),
        (
            "cells",
            list_value(cells.clone().map(|slot| Value::Number(slot.offset as f64))),
        ),
        (
            "measured",
            // A pad playing the shared row has nothing measured of its own.
            list_value(cells.map(|slot| Value::Bool(own && slot.source.is_measured()))),
        ),
    ])
}

/// The heatmap of the groove a rack plays through: an "All" row (the shared
/// row) then one row per pad in pad-note order.
fn heatmap(rack: &ProjectRackConfig, groove: &ProjectGroove) -> Value {
    let mut pads: Vec<i32> = rack.pads.iter().map(|pad| pad.pad_note).collect();
    pads.sort_unstable();
    let mut rows = vec![heat_row("All".to_string(), None, true, groove)];
    for note in pads {
        rows.push(heat_row(
            drum_rack_pad_label(note),
            Some(note),
            groove.pad_row(note).is_some(),
            groove,
        ));
    }
    let slots = groove.slot_count() * heat_repeats(groove);
    map_value([
        ("slots", Value::Number(slots as f64)),
        ("period-beats", Value::Number(groove.period_beats)),
        ("resolution-beats", Value::Number(groove.resolution_beats)),
        ("rows", list_value(rows.into_iter())),
    ])
}

fn resolution_label(resolution_beats: f64) -> &'static str {
    if (resolution_beats - 0.125).abs() < 1e-9 {
        "1/32"
    } else if (resolution_beats - 0.25).abs() < 1e-9 {
        "1/16"
    } else if (resolution_beats - 0.5).abs() < 1e-9 {
        "1/8"
    } else {
        "grid"
    }
}

/// A short "1 bar · 1/16" description of a groove's grid.
pub(crate) fn groove_grid_label(groove: &ProjectGroove) -> String {
    let bars = groove.period_beats / GROOVE_PERIOD_ONE_BAR;
    let period = if (bars - 1.0).abs() < 1e-9 {
        "1 bar".to_string()
    } else if (bars - bars.round()).abs() < 1e-9 && bars >= 1.0 {
        format!("{} bars", bars.round() as u32)
    } else {
        format!("{} beats", groove.period_beats)
    };
    format!("{period} · {}", resolution_label(groove.resolution_beats))
}

fn rack_groove_entry(group: &ProjectTrackGroup, rack: &ProjectRackConfig) -> Value {
    let (labels, keys) = picker(rack);
    let active = rack.groove.active.as_ref();
    let resolved = rack.resolved_active_groove();
    let active_key = match (active, resolved) {
        (Some(active), Some(_)) => active.picker_key(),
        _ => "off".to_string(),
    };
    let active_label = keys
        .iter()
        .position(|key| *key == active_key)
        .map(|index| labels[index].clone())
        .unwrap_or_else(|| GROOVE_PICKER_OFF.to_string());
    let active_rack_id = match active {
        Some(GrooveRef::Rack(id)) if resolved.is_some() => *id as f64,
        _ => -1.0,
    };
    map_value([
        ("group-id", Value::Number(group.id as f64)),
        ("active-key", Value::String(active_key.into())),
        ("active-label", Value::String(active_label.into())),
        // The active groove's id when it is one of the rack's own (rename /
        // delete act on it), else -1.
        ("active-rack-id", Value::Number(active_rack_id)),
        (
            "active-grid",
            Value::String(resolved.map(groove_grid_label).unwrap_or_default().into()),
        ),
        (
            "picker-labels",
            list_value(labels.into_iter().map(|label| Value::String(label.into()))),
        ),
        (
            "picker-keys",
            list_value(keys.into_iter().map(|key| Value::String(key.into()))),
        ),
        (
            "grooves",
            list_value(rack.grooves.iter().map(|groove| {
                map_value([
                    ("id", Value::Number(groove.id as f64)),
                    ("name", Value::String(groove.name.clone().into())),
                    ("grid", Value::String(groove_grid_label(groove).into())),
                ])
            })),
        ),
        (
            "heatmap",
            resolved.map_or(Value::Nil, |groove| heatmap(rack, groove)),
        ),
    ])
}

pub(crate) fn build_rack_grooves_value(groups: &[ProjectTrackGroup]) -> Value {
    list_value(groups.iter().filter_map(|group| {
        group
            .rack
            .as_ref()
            .map(|rack| rack_groove_entry(group, rack))
    }))
}

fn build_groove_builtins_value() -> Value {
    list_value(builtin_grooves().iter().map(|builtin| {
        map_value([
            (
                "key",
                Value::String(GrooveRef::Builtin(builtin.id.clone()).picker_key().into()),
            ),
            ("name", Value::String(builtin.groove.name.clone().into())),
        ])
    }))
}

/// Publishes only the amount fields: what a knob drag changes.
pub(crate) fn sync_rack_groove_amount_fields(rt: &mut Runtime, groups: &[ProjectTrackGroup]) {
    for group in groups {
        let Some(rack) = group.rack.as_ref() else {
            continue;
        };
        let settings = &rack.groove;
        for (amount, value) in [
            ("timing", settings.timing_amount),
            ("velocity", settings.velocity_amount),
            ("random", settings.random_amount),
        ] {
            rt.set_reactive(
                "SEQ",
                &rack_groove_amount_field(amount, group.id),
                Value::Number(value as f64),
            );
        }
    }
}

/// Publishes every rack groove field. Called wherever `SEQ.groups` is.
pub(crate) fn sync_rack_groove_state(rt: &mut Runtime, groups: &[ProjectTrackGroup]) {
    rt.set_reactive("SEQ", "rack-grooves", build_rack_grooves_value(groups));
    rt.set_reactive("SEQ", "groove-builtins", build_groove_builtins_value());
    sync_rack_groove_amount_fields(rt, groups);
}
