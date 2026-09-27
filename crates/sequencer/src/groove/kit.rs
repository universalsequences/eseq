//! What a kit preset carries of its rack's groove (docs/rack-groove-spec.md
//! §Three tiers, "Kit presets", bead eseq-groove.9).
//!
//! Kit version 6 carries a COPY of the rack's active groove plus the rack's
//! amounts ([`KitGroove`]), not a list: loading the kit imports that copy
//! into the project pool through the same dedupe as copy-on-apply.
//!
//! Kit version 5 (rev 1, never shipped) carried the rack's whole groove list
//! plus a selection that pointed into it or at a built-in MPC swing. Such a
//! kit loads by importing its SELECTED groove only
//! (`ProjectKitPreset::carried_groove`); the rest of its list is dropped.

use serde::{Deserialize, Serialize};

use super::{GrooveId, ProjectGroove, RackGroovePad, RackGrooveSettings};

/// Kit v6: a copy of the rack's active groove and the rack's amounts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KitGroove {
    /// The groove's `id` is its pool id at export and means nothing on load.
    pub groove: ProjectGroove,
    #[serde(default = "one")]
    pub timing_amount: f32,
    #[serde(default)]
    pub velocity_amount: f32,
    #[serde(default)]
    pub random_amount: f32,
    /// The rack's on/off switch and per-pad shares (rack groove buffer);
    /// kits written before them load with the groove on and every pad full.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pads: Vec<RackGroovePad>,
    /// The rack's groove time scale (1 for kits written before it).
    #[serde(default = "unit", skip_serializing_if = "is_unit")]
    pub scale: f32,
}

fn unit() -> f32 {
    1.0
}

fn is_unit(value: &f32) -> bool {
    *value == 1.0
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn one() -> f32 {
    1.0
}

impl KitGroove {
    /// What a kit saved from a rack with `settings` carries: the active pool
    /// groove from `pool` plus the amounts, or `None` when the rack plays no
    /// groove.
    pub fn from_rack(settings: &RackGrooveSettings, pool: &[ProjectGroove]) -> Option<Self> {
        let groove = super::pool_groove(pool, settings.active?)?.clone();
        Some(Self {
            groove,
            timing_amount: settings.timing_amount,
            velocity_amount: settings.velocity_amount,
            random_amount: settings.random_amount,
            enabled: settings.enabled,
            pads: settings.pads.clone(),
            scale: settings.scale,
        })
    }

    /// The rack settings this kit groove installs once its copy landed in the
    /// pool as `active` (sanitized).
    pub fn settings(&self, active: Option<GrooveId>) -> RackGrooveSettings {
        let mut settings = RackGrooveSettings {
            active,
            timing_amount: self.timing_amount,
            velocity_amount: self.velocity_amount,
            random_amount: self.random_amount,
            enabled: self.enabled,
            pads: self.pads.clone(),
            scale: self.scale,
        };
        settings.sanitize();
        settings
    }
}

/// A kit v5 groove reference: one of the kit's own grooves, or a rev-1
/// built-in (`mpc-16-58`, `mpc-8-66`, ...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyGrooveRef {
    Rack(GrooveId),
    Builtin(String),
}

/// A kit v5 `groove` value: the rack's selection among the kit's `grooves`
/// list, and its amounts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyKitGrooveSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<LegacyGrooveRef>,
    #[serde(default = "one")]
    pub timing_amount: f32,
    #[serde(default)]
    pub velocity_amount: f32,
    #[serde(default)]
    pub random_amount: f32,
}

/// A kit's `groove` key, in either generation. Untagged: the v6 shape is the
/// one with a `groove` object; anything else is a v5 selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KitGrooveField {
    /// Kit version 6+.
    Copy(KitGroove),
    /// Kit version 5.
    V5Selection(LegacyKitGrooveSettings),
}

/// A rev-1 built-in MPC swing id (`mpc-16-58`, `mpc-8-66`) as the groove it
/// named, so a v5 kit that selected one keeps its feel. The same groove now
/// ships as a factory library file.
pub fn legacy_builtin_groove(id: &str) -> Option<ProjectGroove> {
    let rest = id.strip_prefix("mpc-")?;
    let (label, percent) = rest.split_once('-')?;
    let resolution = match label {
        "16" => 0.25,
        "8" => 0.5,
        _ => return None,
    };
    let percent: u32 = percent.parse().ok()?;
    super::MPC_SWING_PERCENTS
        .contains(&percent)
        .then(|| super::mpc_swing_groove(percent, resolution))
}

/// Resolves a v5 selection against the v5 kit's groove list.
pub(crate) fn resolve_v5_selection(
    selection: &LegacyKitGrooveSettings,
    grooves: &[ProjectGroove],
) -> Option<KitGroove> {
    let groove = match selection.active.as_ref()? {
        LegacyGrooveRef::Rack(id) => super::pool_groove(grooves, *id)?.clone(),
        LegacyGrooveRef::Builtin(id) => legacy_builtin_groove(id)?,
    };
    Some(KitGroove {
        groove,
        timing_amount: selection.timing_amount,
        velocity_amount: selection.velocity_amount,
        random_amount: selection.random_amount,
        enabled: true,
        pads: Vec::new(),
        scale: 1.0,
    })
}
