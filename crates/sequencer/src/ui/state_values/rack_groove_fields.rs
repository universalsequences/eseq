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
use sequencer::groove::{
    GrooveChoice, GrooveLibraryEntry, GrooveLibraryTier, ProjectGroove, GROOVE_PERIOD_ONE_BAR,
};
use sequencer::project::{ProjectRackConfig, ProjectTrackGroup};

/// Picker label for "no groove".
pub(crate) const GROOVE_PICKER_OFF: &str = "No groove";
/// The picker's section headers: dropdown `:headers` rows, drawn dimmed and
/// never picked, with no key of their own.
pub(crate) const GROOVE_PICKER_PROJECT_HEADER: &str = "This project";
pub(crate) const GROOVE_PICKER_FACTORY_HEADER: &str = "Factory";
pub(crate) const GROOVE_PICKER_LIBRARY_HEADER: &str = "Library";

fn reserved_picker_label(label: &str) -> bool {
    [
        GROOVE_PICKER_OFF,
        GROOVE_PICKER_PROJECT_HEADER,
        GROOVE_PICKER_FACTORY_HEADER,
        GROOVE_PICKER_LIBRARY_HEADER,
    ]
    .contains(&label)
}

pub(crate) fn rack_groove_amount_field(amount: &str, group_id: u64) -> String {
    format!("rack-groove-{amount}-{group_id}")
}

/// A clip's own groove publishes its amounts under their own names (the
/// rack's keep the plain ones), so dragging one never moves another.
fn clip_suffix(clip: Option<u64>) -> String {
    clip.map(|clip| format!("-c{clip}")).unwrap_or_default()
}

pub(crate) fn clip_groove_amount_field(amount: &str, group_id: u64, clip: Option<u64>) -> String {
    format!("{}{}", rack_groove_amount_field(amount, group_id), clip_suffix(clip))
}

/// The scalar field one pad's share (0..1) is published on, so dragging the
/// buffer's Amt picker never rebuilds the lanes it sits in.
pub(crate) fn rack_groove_pad_amount_field(group_id: u64, pad_note: i32) -> String {
    format!("rack-groove-pad-{group_id}-{pad_note}")
}

fn clip_pad_amount_field(group_id: u64, clip: Option<u64>, pad_note: i32) -> String {
    format!("{}{}", rack_groove_pad_amount_field(group_id, pad_note), clip_suffix(clip))
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

/// No groove first, then the project pool under *This project*, the
/// factory files under *Factory* and the user's files under *Library*
/// (picking a file copies it into the pool). Labels are made unique (a
/// duplicate pool name, or one spelled like a header or No groove, gets its
/// id; a file name already taken gets "(library)"), so the dropdown's label
/// round-trips to exactly one key.
pub(crate) fn picker(pool: &[ProjectGroove], library: &[GrooveLibraryEntry]) -> GroovePicker {
    let mut labels: Vec<String> = vec![GROOVE_PICKER_OFF.to_string()];
    let mut keys = vec![GrooveChoice::Off.picker_key()];
    let mut headers = Vec::new();
    let mut header = |labels: &mut Vec<String>, keys: &mut Vec<String>, name: &str| {
        headers.push(labels.len());
        labels.push(name.to_string());
        keys.push(String::new());
    };
    if !pool.is_empty() {
        header(&mut labels, &mut keys, GROOVE_PICKER_PROJECT_HEADER);
    }
    for groove in pool {
        let mut label = groove.name.clone();
        if label.is_empty() || labels.contains(&label) || reserved_picker_label(&label) {
            label = format!("{} #{}", groove.name, groove.id);
        }
        labels.push(label);
        keys.push(GrooveChoice::Pool(groove.id).picker_key());
    }
    for (tier, name) in [
        (GrooveLibraryTier::Factory, GROOVE_PICKER_FACTORY_HEADER),
        (GrooveLibraryTier::User, GROOVE_PICKER_LIBRARY_HEADER),
    ] {
        let entries: Vec<&GrooveLibraryEntry> =
            library.iter().filter(|entry| entry.tier == tier).collect();
        if entries.is_empty() {
            continue;
        }
        header(&mut labels, &mut keys, name);
        for entry in entries {
            let mut label = entry.name.clone();
            if labels.contains(&label) || reserved_picker_label(&label) {
                label = format!("{label} (library)");
            }
            if labels.contains(&label) {
                label = format!("{} ({}:{})", entry.name, entry.tier.key(), entry.stem);
            }
            labels.push(label);
            keys.push(entry.choice().picker_key());
        }
    }
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
    [
        (0.0625, "1/64"),
        (0.125, "1/32"),
        (0.25, "1/16"),
        (0.5, "1/8"),
        (1.0, "1/4"),
    ]
    .into_iter()
    .find(|(beats, _)| (resolution_beats - beats).abs() < 1e-9)
    .map_or("grid", |(_, label)| label)
}

/// A short "1 bar · 1/16" description of a groove's grid.
pub(crate) fn groove_grid_label(groove: &ProjectGroove) -> String {
    let bars = groove.period_beats / GROOVE_PERIOD_ONE_BAR;
    let period = if (bars - 1.0).abs() < 1e-9 {
        "1 bar".to_string()
    } else if (bars - bars.round()).abs() < 1e-9 && bars >= 1.0 {
        format!("{} bars", bars.round() as u32)
    } else {
        match groove.period_beats {
            beats if (beats - 1.0).abs() < 1e-9 => "1 beat".to_string(),
            beats => format!("{beats} beats"),
        }
    };
    format!("{period} · {}", resolution_label(groove.resolution_beats))
}

/// Slots in one bar of lane cells when the rack plays straight: the
/// buffer's hit dots read the members' first 16 steps.
const STRAIGHT_LANE_SLOTS: usize = 16;

fn lane_cells(row: &sequencer::groove::GrooveRow, repeats: usize) -> (Value, Value) {
    let cells = (0..repeats).flat_map(|_| row.slots.iter());
    (
        list_value(cells.clone().map(|slot| Value::Number(slot.offset as f64))),
        list_value(cells.map(|slot| Value::Bool(slot.source.is_measured()))),
    )
}

/// The rack groove buffer's lanes: the shared ("All") row and one row per
/// pad in pad-note order, each with the groove row that pad actually plays
/// (`resolve_pad_row`, else the shared row) tiled out to a bar, plus the
/// pad's include flag. With no groove the cells are empty and `slots` is a
/// bar of 16ths, so the buffer draws the pads' hits on the grid instead.
fn rack_groove_lanes(
    group: &ProjectTrackGroup,
    rack: &ProjectRackConfig,
    groove: Option<&ProjectGroove>,
    settings: &sequencer::groove::RackGrooveSettings,
    clip: Option<u64>,
) -> Value {
    let repeats = groove.map_or(1, heat_repeats);
    let slots = groove.map_or(STRAIGHT_LANE_SLOTS, |groove| groove.slot_count() * repeats);
    let empty = || (list_value(std::iter::empty()), list_value(std::iter::empty()));
    let (all_cells, all_measured) =
        groove.map_or_else(empty, |groove| lane_cells(&groove.shared_row, repeats));
    let mut pads: Vec<&sequencer::project::ProjectRackPad> = rack.pads.iter().collect();
    pads.sort_by_key(|pad| pad.pad_note);
    let pad_rows = pads.into_iter().map(|pad| {
        let (cells, measured) = groove.map_or_else(empty, |groove| {
            lane_cells(
                groove.row_for_pad(pad.pad_note, pad.effective_role()),
                repeats,
            )
        });
        let track = group.members.get(pad.member).copied();
        let share = settings.pad(pad.pad_note);
        map_value([
            ("pad-note", Value::Number(pad.pad_note as f64)),
            ("label", Value::String(drum_rack_pad_label(pad.pad_note).into())),
            // Only a role the user set on the pad: the lanes never label a
            // pad by what the standard layout guesses from its note.
            (
                "role-tag",
                Value::String(pad.role.map_or("", |role| role.tag()).into()),
            ),
            (
                "role-label",
                Value::String(pad.role.map_or("", |role| role.label()).into()),
            ),
            ("track", Value::Number(track.map_or(-1.0, |track| track as f64))),
            ("enabled", Value::Bool(share.enabled)),
            (
                "amount-field",
                Value::String(clip_pad_amount_field(group.id, clip, pad.pad_note).into()),
            ),
            ("cells", cells),
            ("measured", measured),
        ])
    });
    map_value([
        ("slots", Value::Number(slots as f64)),
        ("all-cells", all_cells),
        ("all-measured", all_measured),
        ("pads", list_value(pad_rows.collect::<Vec<_>>().into_iter())),
    ])
}

/// The dim text beside each picker row, parallel to its keys: where else a
/// project groove plays ("on Tape Kit", "on 2 racks"), else its grid.
fn picker_details(
    picker: &GroovePicker,
    group: &ProjectTrackGroup,
    groups: &[ProjectTrackGroup],
    pool: &[ProjectGroove],
) -> Vec<String> {
    picker
        .keys
        .iter()
        .map(|key| {
            let Ok(GrooveChoice::Pool(id)) = GrooveChoice::from_picker_key(key) else {
                return String::new();
            };
            let others: Vec<&str> = groups
                .iter()
                .filter(|other| other.id != group.id)
                .filter(|other| {
                    other
                        .rack
                        .as_ref()
                        .is_some_and(|rack| rack.groove.active == Some(id))
                })
                .map(|other| other.name.as_str())
                .collect();
            match others.as_slice() {
                [] => sequencer::groove::pool_groove(pool, id)
                    .map(groove_grid_label)
                    .unwrap_or_default(),
                [one] => format!("on {one}"),
                many => format!("on {} racks", many.len()),
            }
        })
        .collect()
}

/// The Scale dropdown's label for a groove time scale.
pub(crate) fn groove_scale_label(scale: f32) -> &'static str {
    if scale == 0.5 {
        "½×"
    } else if scale == 2.0 {
        "2×"
    } else {
        "1×"
    }
}

/// The grid a groove plays at on a rack: its own, stretched by the rack's
/// Scale ("2 bars · 1/8" for a 1 bar · 1/16 groove at 2×).
fn scaled_grid_label(groove: &ProjectGroove, scale: f32) -> String {
    let mut scaled = groove.clone();
    scaled.period_beats *= scale as f64;
    scaled.resolution_beats *= scale as f64;
    groove_grid_label(&scaled)
}

/// What the buffer shows for one groove setting of a rack: the rack's own
/// (`clip` None) or one clip's.
fn rack_groove_view(
    group: &ProjectTrackGroup,
    groups: &[ProjectTrackGroup],
    rack: &ProjectRackConfig,
    pool: &[ProjectGroove],
    picker: &GroovePicker,
    settings: &sequencer::groove::RackGrooveSettings,
    clip: Option<u64>,
) -> Vec<(&'static str, Value)> {
    let details = picker_details(picker, group, groups, pool);
    let GroovePicker {
        labels,
        keys,
        headers,
    } = picker.clone();
    let resolved = settings
        .active
        .and_then(|id| sequencer::groove::pool_groove(pool, id));
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
    vec![
        ("group-id", Value::Number(group.id as f64)),
        // The clip this view edits (-1: the rack's own groove).
        ("clip-id", Value::Number(clip.map_or(-1.0, |clip| clip as f64))),
        ("active-key", Value::String(active_key.into())),
        ("active-label", Value::String(active_label.into())),
        // The active pool groove's id (rename / delete act on it), else -1.
        ("active-groove-id", Value::Number(active_groove_id)),
        // The buffer's on/off switch; a bypassed groove keeps its selection.
        ("enabled", Value::Bool(settings.enabled)),
        ("lanes", rack_groove_lanes(group, rack, resolved, settings, clip)),
        (
            "active-grid",
            Value::String(
                resolved
                    .map(|groove| scaled_grid_label(groove, settings.scale))
                    .unwrap_or_default()
                    .into(),
            ),
        ),
        ("scale", Value::Number(settings.scale as f64)),
        ("scale-label", Value::String(groove_scale_label(settings.scale).into())),
        (
            "amount-fields",
            map_value(["timing", "velocity", "random"].map(|amount| {
                (
                    amount,
                    Value::String(clip_groove_amount_field(amount, group.id, clip).into()),
                )
            })),
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
            "picker-details",
            list_value(details.into_iter().map(|detail| Value::String(detail.into()))),
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
    ]
}

/// One rack's entry: the rack's own groove at the top level, plus
/// `clip-grooves`, the same view for each clip that plays its own. The
/// buffer shows the one the playing clip uses.
fn rack_groove_entry(
    group: &ProjectTrackGroup,
    groups: &[ProjectTrackGroup],
    rack: &ProjectRackConfig,
    pool: &[ProjectGroove],
    picker: &GroovePicker,
) -> Value {
    let mut entry = rack_groove_view(group, groups, rack, pool, picker, &rack.groove, None);
    entry.push((
        "clip-grooves",
        list_value(rack.clip_grooves.iter().map(|own| {
            map_value(rack_groove_view(
                group,
                groups,
                rack,
                pool,
                picker,
                &own.settings,
                Some(own.clip),
            ))
        })),
    ));
    map_value(entry)
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
            .map(|rack| rack_groove_entry(group, groups, rack, pool, &picker))
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
        let owned = rack.clip_grooves.iter().map(|own| (&own.settings, Some(own.clip)));
        for (settings, clip) in std::iter::once((&rack.groove, None)).chain(owned) {
            for (amount, value) in [
                ("timing", settings.timing_amount),
                ("velocity", settings.velocity_amount),
                ("random", settings.random_amount),
            ] {
                rt.set_reactive(
                    "SEQ",
                    &clip_groove_amount_field(amount, group.id, clip),
                    Value::Number(value as f64),
                );
            }
            for pad in &rack.pads {
                rt.set_reactive(
                    "SEQ",
                    &clip_pad_amount_field(group.id, clip, pad.pad_note),
                    Value::Number(settings.pad(pad.pad_note).amount as f64),
                );
            }
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
