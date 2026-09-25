//! Rack grooves on the drum rack panel (docs/rack-groove-spec.md, "UI").
//!
//! Two kinds of fields, split so a knob drag never rebuilds the section it
//! is dragging in:
//!
//! - `SEQ.rack-grooves` is STRUCTURAL: one entry per drum rack with the
//!   project pool's grooves, the picker (labels + parallel keys + header
//!   indices) and the active groove. It changes on extract / pick / rename /
//!   delete. The rack panel no longer draws a heatmap (eseq-groove.12): a
//!   groove's map is the Grooves tab's preview, on `SEQ.groove-pool`.
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
/// The picker's section header above the library entries: a dropdown
/// `:headers` row, drawn dimmed and never picked, with no key of its own.
pub(crate) const GROOVE_PICKER_LIBRARY_HEADER: &str = "Library";

pub(crate) fn rack_groove_amount_field(amount: &str, group_id: u64) -> String {
    format!("rack-groove-{amount}-{group_id}")
}

/// The rack panel's groove picker: parallel labels and keys, plus the
/// indices of section-header rows (the dropdown's `:headers`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroovePicker {
    pub(crate) labels: Vec<String>,
    /// A header's key is "" (it picks nothing).
    pub(crate) keys: Vec<String>,
    pub(crate) headers: Vec<usize>,
}

/// Project pool grooves first, then a *Library* header over the library
/// entries (factory, then user; picking one copies it into the pool), then
/// Off. Labels are made unique (a duplicate pool name, or one spelled like
/// the header or Off, gets its id; a library name already taken gets
/// "(library)"), so the dropdown's label round-trips to exactly one key.
pub(crate) fn picker(pool: &[ProjectGroove], library: &[GrooveLibraryEntry]) -> GroovePicker {
    let mut labels: Vec<String> = Vec::new();
    let mut keys = Vec::new();
    let mut headers = Vec::new();
    for groove in pool {
        let mut label = groove.name.clone();
        if label.is_empty()
            || labels.contains(&label)
            || label == GROOVE_PICKER_OFF
            || label == GROOVE_PICKER_LIBRARY_HEADER
        {
            label = format!("{} #{}", groove.name, groove.id);
        }
        labels.push(label);
        keys.push(GrooveChoice::Pool(groove.id).picker_key());
    }
    if !library.is_empty() {
        headers.push(labels.len());
        labels.push(GROOVE_PICKER_LIBRARY_HEADER.to_string());
        keys.push(String::new());
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
    GroovePicker {
        labels,
        keys,
        headers,
    }
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

/// A groove's own heatmap, independent of any rack (the Grooves tab's
/// preview): the "All" (shared) row, then one row per recorded pad row in
/// pad-note order, labelled by its role where the row recorded one, else by
/// its pad note. Same cell shape as the rack panel's map.
pub(crate) fn groove_preview_heatmap(groove: &ProjectGroove) -> Value {
    let repeats = heat_repeats(groove);
    let row_value = |label: String,
                     pad_note: Option<i32>,
                     role: Option<&str>,
                     row: &sequencer::groove::GrooveRow| {
        let cells = (0..repeats).flat_map(|_| row.slots.iter());
        map_value([
            ("label", Value::String(label.into())),
            (
                "pad-note",
                pad_note.map_or(Value::Nil, |note| Value::Number(note as f64)),
            ),
            (
                "role",
                role.map_or(Value::Nil, |role| Value::String(role.to_string().into())),
            ),
            ("own", Value::Bool(true)),
            (
                "cells",
                list_value(cells.clone().map(|slot| Value::Number(slot.offset as f64))),
            ),
            (
                "measured",
                list_value(cells.map(|slot| Value::Bool(slot.source.is_measured()))),
            ),
        ])
    };
    let mut pad_rows: Vec<&sequencer::groove::GroovePadRow> = groove.pad_rows.iter().collect();
    pad_rows.sort_by_key(|row| row.pad_note);
    let mut rows = vec![row_value("All".to_string(), None, None, &groove.shared_row)];
    for pad_row in pad_rows {
        let label = pad_row
            .role
            .map(|role| role.label().to_string())
            .unwrap_or_else(|| drum_rack_pad_label(pad_row.pad_note));
        rows.push(row_value(
            label,
            Some(pad_row.pad_note),
            pad_row.role.map(|role| role.key()),
            &pad_row.row,
        ));
    }
    map_value([
        (
            "slots",
            Value::Number((groove.slot_count() * repeats) as f64),
        ),
        ("period-beats", Value::Number(groove.period_beats)),
        ("resolution-beats", Value::Number(groove.resolution_beats)),
        ("grid", Value::String(groove_grid_label(groove).into())),
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
    picker: &GroovePicker,
) -> Value {
    let GroovePicker {
        labels,
        keys,
        headers,
    } = picker.clone();
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
            "picker-headers",
            list_value(headers.into_iter().map(|index| Value::Number(index as f64))),
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
            ("heatmap", groove_preview_heatmap(groove)),
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
