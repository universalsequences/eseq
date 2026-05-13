use eseqlisp::parser::{ASTParser, Expression, Parser};

pub fn validate_instrument_dsp_source(source: &str) -> Result<(), String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("dsp.lisp parse error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("dsp.lisp AST error: {error:?}"))?;

    let mut direct_mod_refs = Vec::new();
    for expr in &exprs {
        if is_modulator_input_declaration(expr) {
            continue;
        }
        collect_direct_modulator_refs(expr, &mut direct_mod_refs);
    }

    direct_mod_refs.sort();
    direct_mod_refs.dedup();
    if direct_mod_refs.is_empty() {
        return Ok(());
    }

    Err(format!(
        "dsp.lisp directly reads host modulator input(s): {}. Do not use mod1..mod6 in DSP expressions. Declare modulator inputs only when params use @mod true, then read the host-modulated parameter with `(mod param_name)`; the host mod matrix chooses which LFO/envelope/random source drives that param.",
        direct_mod_refs.join(", ")
    ))
}

fn is_modulator_input_declaration(expr: &Expression) -> bool {
    let Expression::List(items) = expr else {
        return false;
    };
    if items.len() < 3 {
        return false;
    }
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "def") {
        return false;
    }
    let Some(Expression::Symbol(name)) = items.get(1) else {
        return false;
    };
    if !is_modulator_symbol(name) {
        return false;
    }
    matches!(
        items.get(2),
        Some(Expression::List(input_items))
            if matches!(input_items.first(), Some(Expression::Symbol(head)) if head == "in")
    )
}

fn collect_direct_modulator_refs(expr: &Expression, refs: &mut Vec<String>) {
    match expr {
        Expression::Symbol(symbol) if is_modulator_symbol(symbol) => refs.push(symbol.clone()),
        Expression::List(items) => {
            for item in items {
                collect_direct_modulator_refs(item, refs);
            }
        }
        _ => {}
    }
}

fn is_modulator_symbol(symbol: &str) -> bool {
    matches!(symbol, "mod1" | "mod2" | "mod3" | "mod4" | "mod5" | "mod6")
}

#[cfg(test)]
mod tests {
    use super::validate_instrument_dsp_source;

    #[test]
    fn allows_modulator_input_declarations_and_mod_accessor() {
        validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def mod1 (in 5 @name mod1 @modulator 1))
            (def mod2 (in 6 @name mod2 @modulator 2))
            (param cutoff @default 900 @min 40 @max 12000 @mod true @mod-mode additive)
            (def filtered (svf (sin (* (phasor pitch) twopi)) (clip (mod cutoff) 40 12000) 1 0))
            (out (* filtered gate velocity) 1 @name audio)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn rejects_direct_modulator_refs_in_dsp_expressions() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def mod1 (in 5 @name mod1 @modulator 1))
            (def mod2 (in 6 @name mod2 @modulator 2))
            (param cutoff @default 900 @min 40 @max 12000 @mod true @mod-mode additive)
            (def cutoff-hz (+ (mod cutoff) (* mod1 2500)))
            (def pw (+ 0.5 (* mod2 0.08)))
            (out (* (sin (* (phasor pitch) twopi)) gate velocity) 1 @name audio)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("mod1"));
        assert!(err.contains("mod2"));
        assert!(err.contains("(mod param_name)"));
    }
}
