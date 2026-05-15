use eseqlisp::parser::{ASTParser, Expression, Parser};

pub fn validate_instrument_dsp_source(source: &str) -> Result<(), String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("dsp.lisp parse error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("dsp.lisp AST error: {error:?}"))?;

    let mut mod_declared_params = Vec::new();
    let mut mod_accessor_params = Vec::new();
    let mut declared_modulator_inputs = Vec::new();
    let mut direct_mod_refs = Vec::new();
    for expr in &exprs {
        if let Some(error) = malformed_param_like_form_error(expr) {
            return Err(error);
        }
        if let Some(name) = declared_modulator_input_name(expr) {
            declared_modulator_inputs.push(name);
        }
        if let Some(param) = param_declared_with_mod(expr) {
            mod_declared_params.push(param);
        }
        collect_mod_accessor_params(expr, &mut mod_accessor_params);
        if is_modulator_input_declaration(expr) {
            continue;
        }
        collect_direct_modulator_refs(expr, &mut direct_mod_refs);
    }

    mod_declared_params.sort();
    mod_declared_params.dedup();
    mod_accessor_params.sort();
    mod_accessor_params.dedup();
    declared_modulator_inputs.sort();
    declared_modulator_inputs.dedup();

    let mut non_mod_params = mod_accessor_params
        .iter()
        .filter(|param| !mod_declared_params.contains(param))
        .cloned()
        .collect::<Vec<_>>();
    non_mod_params.sort();
    non_mod_params.dedup();
    if !non_mod_params.is_empty() {
        let locations = non_mod_params
            .iter()
            .map(|param| param_with_line(source, param))
            .collect::<Vec<_>>();
        let direct_reads = non_mod_params
            .iter()
            .map(|param| {
                let location = first_mod_accessor_line(source, param)
                    .map(|line| format!(" on line {line}"))
                    .unwrap_or_default();
                format!("`(mod {param})`{location} -> `{param}`")
            })
            .collect::<Vec<_>>();
        return Err(format!(
            "dsp.lisp uses `(mod param_name)` for parameter(s) not declared with `@mod true`: {}. `(mod param_name)` is only the host modulation accessor for params declared as host modulation targets. Fix this by reading these ordinary/internal controls directly by name: {}. Do not add `@mod true` merely to satisfy this error; only add `@mod true @mod-mode additive` when the parameter is intentionally exposed to the host modulation matrix.",
            locations.join(", "),
            direct_reads.join(", ")
        ));
    }

    direct_mod_refs.sort();
    direct_mod_refs.dedup();
    if !direct_mod_refs.is_empty() {
        return Err(format!(
            "dsp.lisp directly reads host modulator input(s): {}. Do not use mod1..mod10/ext1..ext4 in DSP expressions. Declare modulator inputs only when params use @mod true, then read the host-modulated parameter with `(mod param_name)`; the host mod matrix chooses which LFO/envelope/random/source drives that param.",
            direct_mod_refs.join(", ")
        ));
    }

    if !mod_declared_params.is_empty() {
        let required = [
            "mod1", "mod2", "mod3", "mod4", "mod5", "mod6", "ext1", "ext2", "ext3", "ext4",
        ];
        let missing = required
            .iter()
            .filter(|name| {
                !declared_modulator_inputs
                    .iter()
                    .any(|declared| declared == *name)
            })
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "dsp.lisp declares host-modulatable parameter(s) with `@mod true` but is missing modulator input declaration(s): {}. Declare all ten host modulation lanes, including `(def ext1 (in 11 @name ext1 @modulator 7))` through `(def ext4 (in 14 @name ext4 @modulator 10))`, when any param uses `@mod true`.",
                missing.join(", ")
            ));
        }
    }

    Ok(())
}

pub fn validate_effect_dsp_source(source: &str) -> Result<(), String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("dsp.lisp parse error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("dsp.lisp AST error: {error:?}"))?;

    let mut mod_accessor_params = Vec::new();
    let mut direct_mod_refs = Vec::new();
    let mut mod_metadata_params = Vec::new();
    let mut modulator_inputs = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    for expr in &exprs {
        if let Some(error) = effect_wrapper_form_error(expr) {
            return Err(error);
        }
        if let Some(error) = malformed_param_like_form_error(expr) {
            return Err(error);
        }
        collect_mod_accessor_params(expr, &mut mod_accessor_params);
        collect_direct_modulator_refs(expr, &mut direct_mod_refs);
        collect_effect_mod_metadata_params(expr, &mut mod_metadata_params);
        collect_effect_modulator_inputs(expr, &mut modulator_inputs);
        if let Some(input) = effect_input_decl(expr) {
            inputs.push(input);
        }
        if let Some(output) = effect_output_decl(expr) {
            outputs.push(output);
        }
    }

    mod_accessor_params.sort();
    mod_accessor_params.dedup();
    if !mod_accessor_params.is_empty() {
        return Err(format!(
            "effects cannot use `(mod param_name)` because effect parameters are not host-modulatable yet. Read these effect parameter(s) directly by name instead: {}.",
            mod_accessor_params.join(", ")
        ));
    }

    direct_mod_refs.sort();
    direct_mod_refs.dedup();
    if !direct_mod_refs.is_empty() {
        return Err(format!(
            "effects cannot read host modulation input(s): {}. Do not declare or use mod1..mod10/ext1..ext4 in effect DSP.",
            direct_mod_refs.join(", ")
        ));
    }

    mod_metadata_params.sort();
    mod_metadata_params.dedup();
    if !mod_metadata_params.is_empty() {
        return Err(format!(
            "effects cannot declare host-modulatable parameters yet. Remove `@mod true`/`@mod-mode` metadata from: {}.",
            mod_metadata_params.join(", ")
        ));
    }

    modulator_inputs.sort();
    modulator_inputs.dedup();
    if !modulator_inputs.is_empty() {
        return Err(format!(
            "effects cannot declare `@modulator` inputs. Remove modulation input declaration(s): {}.",
            modulator_inputs.join(", ")
        ));
    }

    require_named_channel(&inputs, 1, "left", "input")?;
    require_named_channel(&inputs, 2, "right", "input")?;
    require_named_channel(&outputs, 1, "left", "output")?;
    require_named_channel(&outputs, 2, "right", "output")?;

    Ok(())
}

fn effect_wrapper_form_error(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "defeffect") {
        return None;
    }

    Some(
        "effect dsp.lisp must not use a `(defeffect ...)` wrapper. DGenLisp effects are written as top-level forms: `(def in_l (in 1 @name left))`, `(def in_r (in 2 @name right))`, `(param ...)`, DSP `(def ...)` helpers, then `(out ... 1 @name left)` and `(out ... 2 @name right)`. Remove the outer `(defeffect name ... )` form and keep its body as top-level dsp_source."
            .to_string(),
    )
}

fn malformed_param_like_form_error(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    let Some(Expression::Symbol(head)) = items.first() else {
        return None;
    };
    if head == "param" {
        return None;
    }
    if !items.iter().any(is_param_metadata_attribute) {
        return None;
    }

    Some(format!(
        "dsp.lisp has a top-level form that looks like a parameter declaration but does not start with `param`: `({head} ...)`. Every parameter must be declared as `(param name @default value @min value @max value ...)`. Do not use dotted names or generated paths such as `{head}` as operators; replace the malformed form with a normal `(param name ...)` declaration, or delete it if the parameter is already declared."
    ))
}

fn is_param_metadata_attribute(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::Symbol(symbol)
            if matches!(
                symbol.as_str(),
                "@default" | "@min" | "@max" | "@unit" | "@mod" | "@mod-mode"
            )
    )
}

fn param_with_line(source: &str, param: &str) -> String {
    match first_mod_accessor_line(source, param) {
        Some(line) => format!("{param} at line {line}"),
        None => param.to_string(),
    }
}

fn first_mod_accessor_line(source: &str, param: &str) -> Option<usize> {
    let needle = format!("(mod {param}");
    source
        .lines()
        .position(|line| line.contains(&needle))
        .map(|index| index + 1)
}

fn is_modulator_input_declaration(expr: &Expression) -> bool {
    declared_modulator_input_name(expr).is_some()
}

fn declared_modulator_input_name(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    if items.len() < 3 {
        return None;
    }
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "def") {
        return None;
    }
    let Some(Expression::Symbol(name)) = items.get(1) else {
        return None;
    };
    if !is_modulator_symbol(name) {
        return None;
    }
    matches!(
        items.get(2),
        Some(Expression::List(input_items))
            if matches!(input_items.first(), Some(Expression::Symbol(head)) if head == "in")
    )
    .then(|| name.clone())
}

fn param_declared_with_mod(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "param") {
        return None;
    }
    let Some(Expression::Symbol(name)) = items.get(1) else {
        return None;
    };
    items
        .windows(2)
        .any(|window| {
            matches!(
                window,
                [Expression::Symbol(attr), Expression::Symbol(value)]
                    if attr == "@mod" && value == "true"
            )
        })
        .then(|| name.clone())
}

fn collect_mod_accessor_params(expr: &Expression, params: &mut Vec<String>) {
    match expr {
        Expression::List(items) => {
            if matches!(items.first(), Some(Expression::Symbol(head)) if head == "mod") {
                if let [_, Expression::Symbol(param)] = items.as_slice() {
                    params.push(param.clone());
                }
            }
            for item in items {
                collect_mod_accessor_params(item, params);
            }
        }
        Expression::Quasiquote(expr) | Expression::Unquote(expr) => {
            collect_mod_accessor_params(expr, params);
        }
        _ => {}
    }
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
    matches!(
        symbol,
        "mod1"
            | "mod2"
            | "mod3"
            | "mod4"
            | "mod5"
            | "mod6"
            | "mod7"
            | "mod8"
            | "mod9"
            | "mod10"
            | "ext"
            | "ext1"
            | "ext2"
            | "ext3"
            | "ext4"
    )
}

#[derive(Debug, Clone)]
struct NamedChannel {
    channel: usize,
    name: Option<String>,
}

fn collect_effect_mod_metadata_params(expr: &Expression, params: &mut Vec<String>) {
    let Expression::List(items) = expr else {
        return;
    };
    if matches!(items.first(), Some(Expression::Symbol(head)) if head == "param") {
        let name = match items.get(1) {
            Some(Expression::Symbol(name)) => name.clone(),
            _ => "<unnamed>".to_string(),
        };
        if items.iter().any(|item| {
            matches!(
                item,
                Expression::Symbol(symbol) if symbol == "@mod" || symbol == "@mod-mode"
            )
        }) {
            params.push(name);
        }
    }
    for item in items {
        collect_effect_mod_metadata_params(item, params);
    }
}

fn collect_effect_modulator_inputs(expr: &Expression, inputs: &mut Vec<String>) {
    let Expression::List(items) = expr else {
        return;
    };
    if matches!(items.first(), Some(Expression::Symbol(head)) if head == "def") {
        let name = match items.get(1) {
            Some(Expression::Symbol(name)) => name.clone(),
            _ => "<unnamed>".to_string(),
        };
        if expr_contains_symbol(expr, "@modulator") {
            inputs.push(name);
        }
    }
    for item in items {
        collect_effect_modulator_inputs(item, inputs);
    }
}

fn expr_contains_symbol(expr: &Expression, needle: &str) -> bool {
    match expr {
        Expression::Symbol(symbol) => symbol == needle,
        Expression::List(items) => items.iter().any(|item| expr_contains_symbol(item, needle)),
        _ => false,
    }
}

fn effect_input_decl(expr: &Expression) -> Option<NamedChannel> {
    let Expression::List(items) = expr else {
        return None;
    };
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "def") {
        return None;
    }
    let Some(Expression::List(input_items)) = items.get(2) else {
        return None;
    };
    if !matches!(input_items.first(), Some(Expression::Symbol(head)) if head == "in") {
        return None;
    }
    named_channel(input_items)
}

fn effect_output_decl(expr: &Expression) -> Option<NamedChannel> {
    let Expression::List(items) = expr else {
        return None;
    };
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "out") {
        return None;
    }
    named_channel(&items[1..])
}

fn named_channel(items: &[Expression]) -> Option<NamedChannel> {
    let channel = items.iter().find_map(expression_usize)?;
    let name = items.windows(2).find_map(|window| match window {
        [Expression::Symbol(attr), Expression::Symbol(name) | Expression::String(name)]
            if attr == "@name" =>
        {
            Some(name.clone())
        }
        _ => None,
    });
    Some(NamedChannel { channel, name })
}

fn expression_usize(expr: &Expression) -> Option<usize> {
    match expr {
        Expression::Number(value) if *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn require_named_channel(
    channels: &[NamedChannel],
    channel: usize,
    name: &str,
    kind: &str,
) -> Result<(), String> {
    if channels
        .iter()
        .any(|candidate| candidate.channel == channel && candidate.name.as_deref() == Some(name))
    {
        return Ok(());
    }
    Err(format!(
        "effect DSP must declare stereo {kind} channel {channel} with `@name {name}`."
    ))
}

#[cfg(test)]
mod tests {
    use super::{validate_effect_dsp_source, validate_instrument_dsp_source};

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
            (def mod3 (in 7 @name mod3 @modulator 3))
            (def mod4 (in 8 @name mod4 @modulator 4))
            (def mod5 (in 9 @name mod5 @modulator 5))
            (def mod6 (in 10 @name mod6 @modulator 6))
            (def ext1 (in 11 @name ext1 @modulator 7))
            (def ext2 (in 12 @name ext2 @modulator 8))
            (def ext3 (in 13 @name ext3 @modulator 9))
            (def ext4 (in 14 @name ext4 @modulator 10))
            (param cutoff @default 900 @min 40 @max 12000 @mod true @mod-mode additive)
            (def filtered (svf (sin (* (phasor pitch) twopi)) (clip (mod cutoff) 40 12000) 1 0))
            (out (* filtered gate velocity) 1 @name audio)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn effect_validator_accepts_stereo_effect_shape() {
        validate_effect_dsp_source(
            r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (param rate @default 5 @min 0.1 @max 20)
            (param depth @default 0.5 @min 0 @max 1)
            (def p (phasor rate))
            (def lfo (scale (triangle p 0.5) -1 1 (- 1 depth) 1))
            (out (* in_l lfo) 1 @name left)
            (out (* in_r lfo) 2 @name right)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn effect_validator_rejects_host_modulation_forms() {
        let err = validate_effect_dsp_source(
            r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (param depth @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
            (out (* in_l (mod depth)) 1 @name left)
            (out (* in_r mod1) 2 @name right)
            "#,
        )
        .unwrap_err();
        assert!(err.contains("effects cannot use `(mod param_name)`"));
    }

    #[test]
    fn effect_validator_rejects_missing_stereo_outputs() {
        let err = validate_effect_dsp_source(
            r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (param gain @default 1 @min 0 @max 2)
            (out (* in_l gain) 1 @name left)
            "#,
        )
        .unwrap_err();
        assert!(err.contains("stereo output channel 2"));
    }

    #[test]
    fn effect_validator_rejects_defeffect_wrapper_before_channel_errors() {
        let err = validate_effect_dsp_source(
            r#"
            (defeffect bad-delay
              (in 1 @name left)
              (in 2 @name right)
              (param mix @default 0.5 @min 0 @max 1)
              (def l (in 1 @name left))
              (def r (in 2 @name right))
              (out l 1 @name left)
              (out r 2 @name right))
            "#,
        )
        .unwrap_err();

        assert!(err.contains("must not use a `(defeffect ...)` wrapper"));
        assert!(err.contains("keep its body as top-level dsp_source"));
    }

    #[test]
    fn rejects_mod_accessor_for_plain_param() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def mod1 (in 5 @name mod1 @modulator 1))
            (def mod2 (in 6 @name mod2 @modulator 2))
            (def mod3 (in 7 @name mod3 @modulator 3))
            (def mod4 (in 8 @name mod4 @modulator 4))
            (def mod5 (in 9 @name mod5 @modulator 5))
            (def mod6 (in 10 @name mod6 @modulator 6))
            (param lfo_to_pitch @default 0 @min 0 @max 0.14)
            (def pitch_mod (+ pitch (* pitch (sin (* (phasor 1) twopi)) (mod lfo_to_pitch))))
            (out (* (sin (* (phasor pitch_mod) twopi)) gate velocity) 1 @name audio)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("lfo_to_pitch"));
        assert!(err.contains("lfo_to_pitch at line"));
        assert!(err.contains("not declared with `@mod true`"));
        assert!(err.contains("`(mod lfo_to_pitch)` on line"));
        assert!(err.contains("-> `lfo_to_pitch`"));
        assert!(err.contains("Do not add `@mod true` merely to satisfy this error"));
    }

    #[test]
    fn rejects_malformed_param_like_top_level_form() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (mod_env.mod_param.mod_sustain @default 0.0 @min 0 @max 1)
            (param mod_sustain @default 0.0 @min 0 @max 1)
            (out (* (sin (* (phasor pitch) twopi)) gate velocity) 1 @name audio)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("looks like a parameter declaration"));
        assert!(err.contains("does not start with `param`"));
        assert!(err.contains("mod_env.mod_param.mod_sustain"));
        assert!(err.contains("delete it if the parameter is already declared"));
    }

    #[test]
    fn rejects_modulatable_param_without_all_modulator_inputs() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (param cutoff @default 900 @min 40 @max 12000 @mod true @mod-mode additive)
            (def filtered (svf (sin (* (phasor pitch) twopi)) (clip (mod cutoff) 40 12000) 1 0))
            (out (* filtered gate velocity) 1 @name audio)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("@mod true"));
        assert!(err.contains("missing modulator input"));
        assert!(err.contains("mod1"));
        assert!(err.contains("mod6"));
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
