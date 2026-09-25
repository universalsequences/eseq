//! Rack grooves on the drum rack panel (docs/rack-groove-spec.md, "UI").
//!
//! Two kinds of fields, split so a knob drag never rebuilds the section it
//! is dragging in:
//!
//! - `SEQ.rack-grooves` is STRUCTURAL: one entry per drum rack with the
//!   project pool's grooves, the picker (labels + parallel keys), the active
//!   groove and the heatmap of the active groove. It changes on extract /
//!   pick / rename / delete / pad-map edits.
//! - `SEQ.rack-groove-{timing,velocity,random}-<group id>` are the amounts,
//!   scalar fields the Timing / Velocity / Random knobs bind to.
//!
//! Rev 2 (eseq-groove.9): grooves live in the project pool and racks point
//! into it. `SEQ.groove-pool` lists the pool with each groove's instances
//! (the racks playing it); `SEQ.groove-library` lists the factory + user
//! `.groove` files (`{key name tier}`), whose picker entries copy-on-apply.

use super::*;
use sequencer::groove::library::list_groove_library;
use sequencer::groove::{GrooveChoice, GrooveLibraryEntry, ProjectGroove, GROOVE_PERIOD_ONE_BAR};
use sequencer::project::{ProjectRackConfig, ProjectTrackGroup};

/// Picker label for "no groove".
pub(crate) const GROOVE_PICKER_OFF: &str = "Off";

pub(crate) fn rack_groove_amount_field(amount: &str, group_id: u64) -> String {
    format!("rack-groove-{amount}-{group_id}")
}

/// Project pool grooves first, then the library (factory, then user; picking
/// one copies it into the pool), then Off. Labels are made unique (a
/// duplicate pool name gets its id, a library name already in the pool gets
/// "(library)"), so the dropdown's label round-trips to exactly one key.
fn picker(pool: &[ProjectGroove], library: &[GrooveLibraryEntry]) -> (Vec<String>, Vec<String>) {
    let mut labels: Vec<String> = Vec::new();
    let mut keys = Vec::new();
    for groove in pool {
        let mut label = groove.name.clone();
        if label.is_empty() || labels.contains(&label) || label == GROOVE_PICKER_OFF {
            label = format!("{} #{}", groove.name, groove.id);
        }
        labels.push(label);
        keys.push(GrooveChoice::Pool(groove.id).picker_key());
    }
    for entry in library {
        let mut label = entry.name.clone();
        if labels.contains(&label) || label == GROOVE_PICKER_OFF {
            label = format!("{label} (library)");
        }
        if labels.contains(&label) {
            label = format!("{} ({}:{})", entry.name, entry.tier.key(), entry.stem);
        }
        labels.push(label);
        keys.push(entry.choice().picker_key());
    }
    labels.push(GROOVE_PICKER_OFF.to_string());
    keys.push(GrooveChoice::Off.picker_key());
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
fn heat_row(
    label: String,
    pad: Option<(i32, Option<sequencer::project::PadRole>)>,
    own: bool,
    groove: &ProjectGroove,
) -> Value {
    let pad_note = pad.map(|(note, _)| note);
    let row = match pad {
        Some((note, role)) => groove.row_for_pad(note, role),
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
    let mut pads: Vec<(i32, Option<sequencer::project::PadRole>)> = rack
        .pads
        .iter()
        .map(|pad| (pad.pad_note, pad.effective_role()))
        .collect();
    pads.sort_unstable_by_key(|(note, _)| *note);
    let mut rows = vec![heat_row("All".to_string(), None, true, groove)];
    for (note, role) in pads {
        // Same lookup the scheduler uses: a pad plays the row with its note,
        // else the row recorded with its role, else the shared row.
        rows.push(heat_row(
            drum_rack_pad_label(note),
            Some((note, role)),
            groove.resolve_pad_row(note, role).is_some(),
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

fn rack_groove_entry(
    group: &ProjectTrackGroup,
    rack: &ProjectRackConfig,
    pool: &[ProjectGroove],
    picker: &(Vec<String>, Vec<String>),
) -> Value {
    let (labels, keys) = picker.clone();
    let resolved = rack.active_groove(pool);
    let active_key = match resolved {
        Some(groove) => GrooveChoice::Pool(groove.id).picker_key(),
        None => GrooveChoice::Off.picker_key(),
    };
    let active_label = keys
        .iter()
        .position(|key| *key == active_key)
        .map(|index| labels[index].clone())
        .unwrap_or_else(|| GROOVE_PICKER_OFF.to_string());
    let active_groove_id = resolved.map_or(-1.0, |groove| groove.id as f64);
    map_value([
        ("group-id", Value::Number(group.id as f64)),
        ("active-key", Value::String(active_key.into())),
        ("active-label", Value::String(active_label.into())),
        // The active pool groove's id (rename / delete act on it), else -1.
        ("active-groove-id", Value::Number(active_groove_id)),
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
            list_value(pool.iter().map(|groove| {
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

pub(crate) fn build_rack_grooves_value(
    groups: &[ProjectTrackGroup],
    pool: &[ProjectGroove],
    library: &[GrooveLibraryEntry],
) -> Value {
    let picker = picker(pool, library);
    list_value(groups.iter().filter_map(|group| {
        group
            .rack
            .as_ref()
            .map(|rack| rack_groove_entry(group, rack, pool, &picker))
    }))
}

/// `SEQ.groove-pool`: every pool groove with its instances (the racks
/// playing it, with their amounts).
pub(crate) fn build_groove_pool_value(
    groups: &[ProjectTrackGroup],
    pool: &[ProjectGroove],
) -> Value {
    list_value(pool.iter().map(|groove| {
        let instances = groups.iter().filter_map(|group| {
            let rack = group.rack.as_ref()?;
            (rack.groove.active == Some(groove.id)).then(|| {
                map_value([
                    ("group-id", Value::Number(group.id as f64)),
                    ("name", Value::String(group.name.clone().into())),
                    ("timing", Value::Number(rack.groove.timing_amount as f64)),
                    (
                        "velocity",
                        Value::Number(rack.groove.velocity_amount as f64),
                    ),
                    ("random", Value::Number(rack.groove.random_amount as f64)),
                ])
            })
        });
        map_value([
            ("id", Value::Number(groove.id as f64)),
            (
                "key",
                Value::String(GrooveChoice::Pool(groove.id).picker_key().into()),
            ),
            ("name", Value::String(groove.name.clone().into())),
            ("grid", Value::String(groove_grid_label(groove).into())),
            (
                "instances",
                list_value(instances.collect::<Vec<_>>().into_iter()),
            ),
        ])
    }))
}

fn build_groove_library_value(library: &[GrooveLibraryEntry]) -> Value {
    list_value(library.iter().map(|entry| {
        map_value([
            ("key", Value::String(entry.choice().picker_key().into())),
            ("name", Value::String(entry.name.clone().into())),
            ("tier", Value::String(entry.tier.key().into())),
            ("stem", Value::String(entry.stem.clone().into())),
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

/// Publishes every rack groove field. Called wherever `SEQ.groups` is. Reads
/// the groove library only when the project has a drum rack or a pool groove
/// to show it beside, through `list_groove_library`'s cached listing (re-read
/// only when a tier directory changes or the app saves/renames/deletes).
pub(crate) fn sync_rack_groove_state(
    rt: &mut Runtime,
    groups: &[ProjectTrackGroup],
    pool: &[ProjectGroove],
) {
    let library = if groups.iter().any(|group| group.rack.is_some()) || !pool.is_empty() {
        list_groove_library()
    } else {
        Vec::new()
    };
    rt.set_reactive(
        "SEQ",
        "rack-grooves",
        build_rack_grooves_value(groups, pool, &library),
    );
    rt.set_reactive("SEQ", "groove-pool", build_groove_pool_value(groups, pool));
    rt.set_reactive(
        "SEQ",
        "groove-library",
        build_groove_library_value(&library),
    );
    sync_rack_groove_amount_fields(rt, groups);
}
