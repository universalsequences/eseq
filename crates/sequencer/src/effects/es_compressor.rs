//! ES Compressor: three distinct dynamics architectures, hosted as bundled
//! DGenLisp. No external assets or FFT services. See the effect spec for the
//! published equations, original design choices, and numerical derivations.

/// Display name used by the builtin compile/apply path.
pub const NAME: &str = "ES Compressor";

/// Timing controls use exponential knob travel in the host descriptor.
pub const PARAM_ATTACK: &str = "attack";
pub const PARAM_RELEASE: &str = "release";

pub fn dsp_source() -> &'static str {
    include_str!("es_compressor_dsp.lisp")
}

#[cfg(test)]
#[path = "es_compressor_tests.rs"]
mod tests;
