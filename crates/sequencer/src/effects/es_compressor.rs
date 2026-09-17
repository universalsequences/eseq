//! ES Compressor: a sampler-style "sustain" compressor whose DSP body is
//! bundled dgenlisp (`es_compressor_dsp.lisp`), compiled and hosted through
//! the same path as custom effects. Unlike the convolution reverb and Filter
//! Table builtins it carries no per-instance asset state: every curve is a
//! closed-form formula, and its parameters come straight from the compiled
//! manifest. The source declares its own `(effect-latency …)`, which is how
//! the host's delay compensation learns about the overlap-save block delay.

/// Display name of the builtin effect. Recognized in the builtin add path,
/// where it routes through the dgenlisp compile/apply path with bundled source.
pub const NAME: &str = "ES Compressor";

/// Parameters that read best on a logarithmic knob taper.
pub const PARAM_ATTACK: &str = "attack";
pub const PARAM_RELEASE: &str = "release";

/// The bundled dgenlisp DSP source.
pub fn dsp_source() -> &'static str {
    include_str!("es_compressor_dsp.lisp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_source_declares_stereo_io_params_and_latency() {
        let source = dsp_source();
        assert!(source.contains("(in 1 @name Left)"));
        assert!(source.contains("(in 2 @name Right)"));
        assert!(source.contains("(out ") && source.contains(" 2 @name Right)"));
        for param in ["amount", PARAM_ATTACK, PARAM_RELEASE, "mix", "drive", "input-db", "output-db", "detector-db"] {
            assert!(source.contains(&format!("(param {param} ")), "missing param {param}");
        }
        assert!(source.contains("(effect-latency "));
        // Nothing table-driven: the builtin must stay asset-free.
        assert!(!source.contains("@file"));
        assert!(!source.contains("tensor-param"));
    }

    #[test]
    fn declared_latency_resolves_to_block_delay_plus_peak_window() {
        for (rate, expected) in [(44_100u32, 266u32), (48_000, 267), (96_000, 279), (8_000, 257), (384_000, 351)] {
            let latency = crate::lisp_host::declared_effect_latency_samples(dsp_source(), rate)
                .expect("latency declaration parses");
            assert_eq!(latency, Some(expected), "rate {rate}");
        }
    }
}
