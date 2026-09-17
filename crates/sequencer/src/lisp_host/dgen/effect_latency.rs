//! Host-owned effect metadata, deliberately separate from mutable DSP params.
//! The source declaration stays in saved patches/cache keys; only the compiler
//! copy loses it. Its resolved value travels in the cached manifest.

use eseqlisp::parser::{Expr, ExprKind, Parser, SpannedASTParser};

const FORM: &str = "effect-latency";
const MAX_DEPTH: usize = 64;

fn error(message: impl std::fmt::Display) -> String {
    format!("effect-latency: {message}")
}

fn eval(expr: &Expr, rate: u32, depth: usize) -> Result<f64, String> {
    if depth > MAX_DEPTH {
        return Err(error("expression nesting exceeds 64"));
    }
    let value = match &expr.kind {
        ExprKind::Number(value) => *value,
        ExprKind::Symbol(name) if matches!(name.as_str(), "samplerate" | "sample-rate") => rate as f64,
        ExprKind::List(items) => {
            let Some(Expr { kind: ExprKind::Symbol(op), .. }) = items.first() else {
                return Err(error("expected an arithmetic operator"));
            };
            let args = items[1..].iter().map(|arg| eval(arg, rate, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            match (op.as_str(), args.as_slice()) {
                ("+", [_, ..]) => args.iter().sum(),
                ("*", [_, ..]) => args.iter().product(),
                ("-", [a]) => -a,
                ("-", [a, b]) => a - b,
                ("/", [a, b]) if *b != 0.0 => a / b,
                ("min", [a, b]) => a.min(*b),
                ("max", [a, b]) => a.max(*b),
                ("floor", [a]) => a.floor(),
                ("ceil", [a]) => a.ceil(),
                ("round", [a]) => a.round(),
                ("pow", [a, b]) => a.powf(*b),
                ("log2", [a]) => a.log2(),
                _ => return Err(error(format!("invalid operator, arity or divisor: {op}"))),
            }
        }
        _ => return Err(error("only numbers, sample-rate and arithmetic are allowed; no DSP or parameter references")),
    };
    if !value.is_finite() {
        return Err(error("expression must remain finite"));
    }
    Ok(value)
}

fn validate_samples(value: u64) -> Result<u32, String> {
    let max = crate::effects::pdc_delay::PDC_MAX_DELAY_SAMPLES - 1;
    if value > max as u64 {
        return Err(error(format!("{value} samples exceeds PDC capacity ({max} samples)")));
    }
    Ok(value as u32)
}

/// Remove exactly one optional top-level declaration without changing source
/// positions of the remaining program (including UTF-8 byte offsets/newlines).
/// Nested declarations are errors, not silently ignored metadata.
pub(crate) fn prepare(source: &str, rate: u32, is_effect: bool) -> Result<(String, Option<u32>), String> {
    if !source.contains(FORM) {
        return Ok((source.to_string(), None));
    }
    let tokens = Parser::new(source.to_string()).parse_spanned()
        .map_err(|e| error(format!("cannot parse source: {e:?}")))?;
    let forms = SpannedASTParser::new(tokens).parse()
        .map_err(|e| error(format!("cannot parse source: {e:?}")))?;
    let mut declaration = None;
    let mut pending: Vec<_> = forms.iter().map(|form| (form, true)).collect();
    while let Some((form, top_level)) = pending.pop() {
        if let ExprKind::List(items) = &form.kind {
            if matches!(items.first(), Some(Expr { kind: ExprKind::Symbol(name), .. }) if name == FORM) {
                if !is_effect || !top_level {
                    return Err(error("declaration must be at the top level of an audio effect"));
                }
                if declaration.is_some() || items.len() != 2 {
                    return Err(error("expected exactly one declaration with one expression"));
                }
                if rate == 0 {
                    return Err(error("sample rate must be positive"));
                }
                let value = eval(&items[1], rate, 0)?;
                if value < 0.0 || value.fract() != 0.0 || value > u32::MAX as f64 {
                    return Err(error("result must be a nonnegative integer sample count; use an explicit rounding operation"));
                }
                declaration = Some((form.origin.primary_span.clone(), validate_samples(value as u64)?));
            } else {
                pending.extend(items.iter().map(|item| (item, false)));
            }
        }
    }
    let Some((span, samples)) = declaration else {
        return Ok((source.to_string(), None));
    };
    let mut compiler_source = source.as_bytes().to_vec();
    for byte in &mut compiler_source[span.start_byte..span.end_byte] {
        if *byte != b'\n' && *byte != b'\r' { *byte = b' '; }
    }
    Ok((String::from_utf8(compiler_source).expect("replacement preserves UTF-8"), Some(samples)))
}

pub(super) fn annotate_manifest(json: &str, samples: Option<u32>) -> Result<String, String> {
    let Some(samples) = samples else { return Ok(json.to_string()); };
    let mut manifest: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| error(format!("invalid compiler manifest: {e}")))?;
    let object = manifest.as_object_mut().ok_or_else(|| error("manifest must be an object"))?;
    if object.contains_key("eseqEffect") {
        return Err(error("compiler manifest uses reserved host metadata namespace eseqEffect"));
    }
    object.insert("eseqEffect".into(), serde_json::json!({"version": 1, "latencySamples": samples}));
    serde_json::to_string(&manifest).map_err(|e| error(e))
}

pub(super) fn parse_manifest(value: &serde_json::Value) -> Result<Option<u32>, String> {
    let Some(metadata) = value.get("eseqEffect") else { return Ok(None); };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct Metadata { version: u32, latency_samples: u64 }
    let metadata: Metadata = serde_json::from_value(metadata.clone()).map_err(|e| error(e))?;
    if metadata.version != 1 { return Err(error("unsupported host metadata version")); }
    validate_samples(metadata.latency_samples).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_resolve_at_host_rate_and_preserve_source_positions() {
        let source = "; é metadata\n(effect-latency (+ 31 (round (* samplerate 0.001))))\n(out (in 1) 1)";
        for (rate, expected) in [(44100, 75), (48000, 79), (96000, 127)] {
            let (prepared, latency) = prepare(source, rate, true).unwrap();
            assert_eq!(latency, Some(expected));
            assert_eq!(prepared.len(), source.len());
            assert_eq!(prepared.find("(out"), source.find("(out"));
            assert!(prepared.starts_with("; é metadata\n"));
            let json = annotate_manifest("{}", latency).unwrap();
            assert_eq!(parse_manifest(&serde_json::from_str(&json).unwrap()).unwrap(), latency);
        }
        assert_eq!(prepare("(effect-latency 0)", 48000, true).unwrap().1, Some(0));
        assert_eq!(prepare("; (effect-latency 7)\n(out (in 1) 1)", 48000, true).unwrap().1, None);
    }

    #[test]
    fn patcher_writeback_preserves_host_latency_declaration() {
        use eseqlisp::widget_render::patcher::{emit_patch_writeback_source, PatcherIntent};
        let source = "(effect-latency (+ 31 (round (* samplerate 0.001))))\n(def left (in 1))\n(out left 1)";
        let emitted = emit_patch_writeback_source(source, PatcherIntent::Effect).unwrap();
        assert_eq!(prepare(&emitted, 48000, true).unwrap().1, Some(79));
    }

    #[test]
    fn compiled_latency_survives_cache_restart_and_rate_changes() {
        use crate::lisp_host::{DGenCompileKind, DGenSourceOrigin, dylib_cache::DylibCacheManager};
        let dir = tempfile::tempdir().unwrap();
        let source = "(effect-latency (+ 31 (round (* samplerate 0.001))))\n(out (in 1) 1)\n(out (in 2) 2)";
        for (rate, expected) in [(44100, 75), (96000, 127)] {
            let mut artifact = None;
            for _ in 0..2 {
                let manager = DylibCacheManager::new(dir.path().to_path_buf());
                let result = manager.acquire(DGenCompileKind::Effect, DGenSourceOrigin::Custom,
                    source, rate, None).unwrap();
                assert_eq!(result.manifest.effect_latency_samples, Some(expected));
                if let Some(previous) = &artifact { assert_eq!(previous, &result.manifest.dylib_path); }
                artifact = Some(result.manifest.dylib_path.clone());
                let desc = crate::effects::EffectDescriptor::from_lisp_manifest("authored",
                    &result.manifest.params, result.manifest.n_inputs, result.manifest.n_outputs,
                    result.manifest.effect_latency_samples);
                assert_eq!(desc.latency_samples(-1), expected);
                assert_eq!(desc.params.len(), 1, "latency is not a DSP/UI parameter (only enabled is appended)");
            }
        }
        let json = crate::lisp_host::compile_lisp(source, 48000).unwrap();
        assert_eq!(crate::lisp_host::parse_manifest(&json).unwrap().effect_latency_samples, Some(79));
    }

    #[test]
    fn invalid_declarations_and_metadata_fail_loudly() {
        for source in ["(effect-latency -1)", "(effect-latency 1.5)",
            "(effect-latency (/ 1 0))", "(effect-latency (pow 10 999))",
            "(effect-latency amount)", "(effect-latency (in 1))",
            "(effect-latency 16384)", "(effect-latency 1 2)",
            "(effect-latency 1) (effect-latency 2)",
            "(defmacro bad () (effect-latency 1))"] {
            assert!(prepare(source, 48000, true).is_err(), "{source}");
        }
        assert!(prepare("(effect-latency 0)", 48000, false).is_err());
        for metadata in [serde_json::json!({"version":2,"latencySamples":1}),
            serde_json::json!({"version":1,"latencySamples":-1}),
            serde_json::json!({"version":1,"latencySamples":1.5}),
            serde_json::json!({"version":1,"latencySamples":16384})] {
            assert!(parse_manifest(&serde_json::json!({"eseqEffect":metadata})).is_err());
        }
    }
}
