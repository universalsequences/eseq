//! Drum pad roles (docs/rack-groove-spec.md, "Pad roles (typed slots)", bead
//! eseq-groove.10): what drum a rack or kit pad IS, independent of the note
//! it sits on.
//!
//! `pad_note` identifies a pad within one kit; across kits it only names the
//! same drum when both kits share a layout. A role is optional pad metadata:
//! set explicitly on a pad (`ProjectRackPad::role` / `ProjectKitPad::role`),
//! or inferred from the pad note through the standard layout — the General
//! MIDI drum map on the rack's home octave, kick on C1 = pad note -36
//! ([`PadRole::standard`]). Grooves
//! are the first consumer (role-aware row lookup); pattern transfer between
//! kits and MIDI note maps can use it later.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PadRole {
    Kick,
    Snare,
    Rim,
    Clap,
    ClosedHat,
    PedalHat,
    OpenHat,
    TomLow,
    TomMid,
    TomHigh,
    Crash,
    Ride,
    Shaker,
    Perc,
}

/// Pad note the standard layout starts on: the rack's home octave, C1
/// (`DRUM_RACK_FIRST_PAD_NOTE`), where `next_free_pad_note` puts a new rack's
/// first pads. Kick C1, snare D1, closed hat F#1 — the GM drum map under the
/// note names drum racks conventionally use.
pub const STANDARD_LAYOUT_FIRST_PAD_NOTE: i32 = crate::sequencer::DRUM_RACK_FIRST_PAD_NOTE;

/// The standard layout, indexed from `STANDARD_LAYOUT_FIRST_PAD_NOTE`: GM
/// notes 36..=56.
const STANDARD_LAYOUT: [PadRole; 21] = [
    PadRole::Kick,      // 0  GM 36 bass drum 1
    PadRole::Rim,       // 1  GM 37 side stick
    PadRole::Snare,     // 2  GM 38 acoustic snare
    PadRole::Clap,      // 3  GM 39 hand clap
    PadRole::Snare,     // 4  GM 40 electric snare
    PadRole::TomLow,    // 5  GM 41 low floor tom
    PadRole::ClosedHat, // 6  GM 42 closed hi-hat
    PadRole::TomLow,    // 7  GM 43 high floor tom
    PadRole::PedalHat,  // 8  GM 44 pedal hi-hat
    PadRole::TomMid,    // 9  GM 45 low tom
    PadRole::OpenHat,   // 10 GM 46 open hi-hat
    PadRole::TomMid,    // 11 GM 47 low-mid tom
    PadRole::TomHigh,   // 12 GM 48 hi-mid tom
    PadRole::Crash,     // 13 GM 49 crash 1
    PadRole::TomHigh,   // 14 GM 50 high tom
    PadRole::Ride,      // 15 GM 51 ride 1
    PadRole::Crash,     // 16 GM 52 chinese cymbal
    PadRole::Ride,      // 17 GM 53 ride bell
    PadRole::Shaker,    // 18 GM 54 tambourine
    PadRole::Crash,     // 19 GM 55 splash
    PadRole::Perc,      // 20 GM 56 cowbell
];

impl PadRole {
    /// Every role, in menu order.
    pub const ALL: [PadRole; 14] = [
        PadRole::Kick,
        PadRole::Snare,
        PadRole::Rim,
        PadRole::Clap,
        PadRole::ClosedHat,
        PadRole::PedalHat,
        PadRole::OpenHat,
        PadRole::TomLow,
        PadRole::TomMid,
        PadRole::TomHigh,
        PadRole::Crash,
        PadRole::Ride,
        PadRole::Shaker,
        PadRole::Perc,
    ];

    /// The role the standard layout gives `pad_note`, or `None` outside it.
    pub fn standard(pad_note: i32) -> Option<PadRole> {
        usize::try_from(pad_note - STANDARD_LAYOUT_FIRST_PAD_NOTE)
            .ok()
            .and_then(|index| STANDARD_LAYOUT.get(index).copied())
    }

    /// An explicit role wins; otherwise the standard layout's.
    pub fn effective(explicit: Option<PadRole>, pad_note: i32) -> Option<PadRole> {
        explicit.or_else(|| PadRole::standard(pad_note))
    }

    /// Stable wire key (the serde name): host commands and `SEQ.groups`.
    pub fn key(self) -> &'static str {
        match self {
            PadRole::Kick => "kick",
            PadRole::Snare => "snare",
            PadRole::Rim => "rim",
            PadRole::Clap => "clap",
            PadRole::ClosedHat => "closed-hat",
            PadRole::PedalHat => "pedal-hat",
            PadRole::OpenHat => "open-hat",
            PadRole::TomLow => "tom-low",
            PadRole::TomMid => "tom-mid",
            PadRole::TomHigh => "tom-high",
            PadRole::Crash => "crash",
            PadRole::Ride => "ride",
            PadRole::Shaker => "shaker",
            PadRole::Perc => "perc",
        }
    }

    pub fn from_key(key: &str) -> Option<PadRole> {
        PadRole::ALL.into_iter().find(|role| role.key() == key)
    }

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            PadRole::Kick => "Kick",
            PadRole::Snare => "Snare",
            PadRole::Rim => "Rim",
            PadRole::Clap => "Clap",
            PadRole::ClosedHat => "Closed Hat",
            PadRole::PedalHat => "Pedal Hat",
            PadRole::OpenHat => "Open Hat",
            PadRole::TomLow => "Low Tom",
            PadRole::TomMid => "Mid Tom",
            PadRole::TomHigh => "High Tom",
            PadRole::Crash => "Crash",
            PadRole::Ride => "Ride",
            PadRole::Shaker => "Shaker",
            PadRole::Perc => "Perc",
        }
    }

    /// Short tag drawn on the pad (drum-machine abbreviations).
    pub fn tag(self) -> &'static str {
        match self {
            PadRole::Kick => "BD",
            PadRole::Snare => "SD",
            PadRole::Rim => "RS",
            PadRole::Clap => "CP",
            PadRole::ClosedHat => "CH",
            PadRole::PedalHat => "PH",
            PadRole::OpenHat => "OH",
            PadRole::TomLow => "LT",
            PadRole::TomMid => "MT",
            PadRole::TomHigh => "HT",
            PadRole::Crash => "CR",
            PadRole::Ride => "RD",
            PadRole::Shaker => "SH",
            PadRole::Perc => "PC",
        }
    }
}
