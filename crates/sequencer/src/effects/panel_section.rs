//! Where a param sits on its device's panel: the main controls, a
//! modulation lane's own params (`mod …`), a modulation source's settings
//! (voice modulator source params), or host plumbing nobody shows. Shared by
//! the app's instrument panel builders and host kinds (`param.section`) and
//! by plugin export, which places params the same way.

use crate::effects::ParamDescriptor;
use crate::instruments::voice_modulator;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSection {
    Main,
    Mod,
    Source,
    Hidden,
}

impl PanelSection {
    pub fn of(pdesc: &ParamDescriptor) -> Self {
        if is_generated_host_mod_param(&pdesc.name) || is_hidden_dgen_mod_param(&pdesc.name) {
            Self::Hidden
        } else if is_source_param(pdesc.node_param_idx) {
            Self::Source
        } else if is_mod_param(&pdesc.name) {
            Self::Mod
        } else {
            Self::Main
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Mod => "mod",
            Self::Source => "source",
            Self::Hidden => "hidden",
        }
    }

    /// The name the panel shows: a mod param without its `mod ` prefix, a
    /// source param by its role (`type`, `rate`, …).
    pub fn label(self, pdesc: &ParamDescriptor) -> String {
        match self {
            Self::Mod => (pdesc.name.strip_prefix("mod ").unwrap_or(&pdesc.name)).to_string(),
            Self::Source => voice_modulator::source_param_display_name(&pdesc.name),
            Self::Main | Self::Hidden => pdesc.name.clone(),
        }
    }

    /// The modulation source (1-based) a source param sets; 0 otherwise.
    pub fn mod_slot(self, pdesc: &ParamDescriptor) -> usize {
        match self {
            Self::Source => voice_modulator::slot_from_param_name(&pdesc.name).unwrap_or(0),
            _ => 0,
        }
    }
}

pub fn is_mod_param(name: &str) -> bool {
    name.starts_with("mod ")
}

pub fn is_generated_host_mod_param(name: &str) -> bool {
    name.starts_with("__host_mod__")
}

pub fn is_hidden_dgen_mod_param(name: &str) -> bool {
    name.starts_with("__dgen_mod_active__")
}

pub fn is_source_param(node_param_idx: u32) -> bool {
    // u32::MAX marks host-only controls such as sampler slicing; it is not a
    // packed voice-modulator source index.
    node_param_idx != u32::MAX && voice_modulator::is_source_param(node_param_idx)
}
