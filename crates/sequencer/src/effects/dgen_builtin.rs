//! Registry for built-in effects whose DSP implementation is compiled DGenLisp.

use crate::lisp_host::DGenSourceOrigin;

#[derive(Clone, Copy)]
pub struct DGenBuiltin {
    pub name: &'static str,
    pub source: &'static str,
    pub origin: DGenSourceOrigin,
}

pub const NAMES: &[&str] = &[super::conv_reverb::NAME, super::filter_table::NAME];

pub fn find(name: &str) -> Option<DGenBuiltin> {
    if name == super::conv_reverb::NAME {
        Some(DGenBuiltin {
            name: super::conv_reverb::NAME,
            source: super::conv_reverb::dsp_source(),
            origin: DGenSourceOrigin::BuiltinConvolutionReverb,
        })
    } else if name == super::filter_table::NAME {
        Some(DGenBuiltin {
            name: super::filter_table::NAME,
            source: super::filter_table::dsp_source(),
            origin: DGenSourceOrigin::BuiltinFilterTable,
        })
    } else {
        None
    }
}

pub fn contains(name: &str) -> bool {
    find(name).is_some()
}

pub fn clear_instance(node_id: i32) {
    super::conv_reverb::clear_instance(node_id);
    super::filter_table::clear_instance(node_id);
}
