//! What the `groove` / `pad-groove` / `pool-groove` / `library-groove`
//! kinds (host_kinds/racks.rs) show of a rack's groove
//! (docs/rack-groove-spec.md, "UI"): a groove's grid label, its lanes and
//! the groove library listing. The *groove* buffer
//! (content/ui/rack-groove-buffer.lisp) builds its picker from the kinds.

use super::*;
use sequencer::groove::library::list_groove_library;
use sequencer::groove::{GrooveLibraryEntry, ProjectGroove, GROOVE_PERIOD_ONE_BAR};
use sequencer::project::{ProjectRackConfig, ProjectTrackGroup};

/// How many times the groove's period repeats across the buffer's lanes: a
/// groove shorter than a bar (the two-slot MPC swings) is tiled out to one
/// bar, so the lanes always read as a bar of the pocket.
fn heat_repeats(groove: &ProjectGroove) -> usize {
    if groove.period_beats >= GROOVE_PERIOD_ONE_BAR - 1e-9 || groove.period_beats <= 0.0 {
        1
    } else {
        ((GROOVE_PERIOD_ONE_BAR / groove.period_beats).round() as usize).max(1)
    }
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
    grid_label(groove.period_beats, groove.resolution_beats)
}

/// [`groove_grid_label`] of a period and resolution, in beats.
fn grid_label(period_beats: f64, resolution_beats: f64) -> String {
    let bars = period_beats / GROOVE_PERIOD_ONE_BAR;
    let period = if (bars - 1.0).abs() < 1e-9 {
        "1 bar".to_string()
    } else if (bars - bars.round()).abs() < 1e-9 && bars >= 1.0 {
        format!("{} bars", bars.round() as u32)
    } else {
        match period_beats {
            beats if (beats - 1.0).abs() < 1e-9 => "1 beat".to_string(),
            beats => format!("{beats} beats"),
        }
    };
    format!("{period} · {}", resolution_label(resolution_beats))
}

/// The grid a groove plays at on a rack: its own, stretched by the rack's
/// Scale ("2 bars · 1/8" for a 1 bar · 1/16 groove at 2×).
pub(crate) fn scaled_grid_label(groove: &ProjectGroove, scale: f32) -> String {
    let scale = scale as f64;
    grid_label(groove.period_beats * scale, groove.resolution_beats * scale)
}

/// Slots in one bar of lane cells when the rack plays straight: the
/// buffer's hit dots read the members' first 16 steps.
const STRAIGHT_LANE_SLOTS: usize = 16;

/// One lane of the groove buffer: where each slot of a bar lands (offset
/// from the grid, in slots: late positive, early negative) and whether the
/// slot was measured rather than filled.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GrooveLane {
    pub(crate) offsets: Vec<f64>,
    pub(crate) measured: Vec<bool>,
}

impl GrooveLane {
    fn of(row: &sequencer::groove::GrooveRow, repeats: usize) -> Self {
        let slots = (0..repeats).flat_map(|_| row.slots.iter());
        Self {
            offsets: slots.clone().map(|slot| slot.offset as f64).collect(),
            measured: slots.map(|slot| slot.source.is_measured()).collect(),
        }
    }

    pub(crate) fn values(&self) -> (Value, Value) {
        (
            list_value(self.offsets.iter().map(|offset| Value::Number(*offset))),
            list_value(self.measured.iter().map(|measured| Value::Bool(*measured))),
        )
    }
}

/// The groove buffer's lanes for a rack playing `groove` (`None`: straight):
/// the slot count of a bar, the shared ("All") lane and one lane per pad in
/// pad-note order, each the groove row that pad actually plays
/// (`row_for_pad`, else the shared row) tiled out to a bar. With no groove
/// the lanes are empty and `slots` is a bar of 16ths, so the buffer draws
/// the pads' hits on the grid instead: the `groove` / `pad-groove` kinds'
/// lanes.
pub(crate) struct GrooveLanes<'a> {
    pub(crate) slots: usize,
    pub(crate) all: GrooveLane,
    pub(crate) pads: Vec<(&'a sequencer::project::ProjectRackPad, GrooveLane)>,
}

pub(crate) fn groove_lanes<'a>(
    rack: &'a ProjectRackConfig,
    groove: Option<&ProjectGroove>,
) -> GrooveLanes<'a> {
    let repeats = groove.map_or(1, heat_repeats);
    let slots = groove.map_or(STRAIGHT_LANE_SLOTS, |groove| groove.slot_count() * repeats);
    let all = groove.map_or_else(GrooveLane::default, |groove| {
        GrooveLane::of(&groove.shared_row, repeats)
    });
    let mut pads: Vec<&sequencer::project::ProjectRackPad> = rack.pads.iter().collect();
    pads.sort_by_key(|pad| pad.pad_note);
    let pads = pads
        .into_iter()
        .map(|pad| {
            let lane = groove.map_or_else(GrooveLane::default, |groove| {
                GrooveLane::of(
                    groove.row_for_pad(pad.pad_note, pad.effective_role()),
                    repeats,
                )
            });
            (pad, lane)
        })
        .collect();
    GrooveLanes { slots, all, pads }
}

/// The groove library as the rack groove pickers list it
/// (`project.groove-library`): read (through `list_groove_library`'s cached
/// listing) only when the project has a drum rack or a pool groove to show
/// it beside; empty otherwise.
pub(crate) fn listed_groove_library(
    groups: &[ProjectTrackGroup],
    pool: &[ProjectGroove],
) -> Vec<GrooveLibraryEntry> {
    if groups.iter().any(|group| group.rack.is_some()) || !pool.is_empty() {
        list_groove_library()
    } else {
        Vec::new()
    }
}
