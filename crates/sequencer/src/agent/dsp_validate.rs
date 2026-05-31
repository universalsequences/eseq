use eseqlisp::parser::{ASTParser, Expression, Parser};

pub fn validate_instrument_dsp_source(source: &str) -> Result<(), String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("dsp.lisp parse error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("dsp.lisp AST error: {error:?}"))?;

    validate_forward_references(source, &exprs)?;

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
            "dsp.lisp directly reads host modulator input(s): {}. Do not use mod1..mod4 or legacy mod5..mod10/ext1..ext4 in DSP expressions. Declare modulator inputs only when params use @mod true, then read the host-modulated parameter with `(mod param_name)`; the host mod matrix chooses which configured Mod slot drives that param.",
            direct_mod_refs.join(", ")
        ));
    }

    let required = ["mod1", "mod2", "mod3", "mod4"];
    let mut extra = declared_modulator_inputs
        .iter()
        .filter(|name| !required.iter().any(|required| *required == name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    extra.sort();
    extra.dedup();
    if !extra.is_empty() {
        return Err(format!(
            "dsp.lisp declares unsupported host modulator input(s): {}. Instruments expose exactly four configurable host modulation slots: `(def mod1 (in 6 @name mod1 @modulator 1))` through `(def mod4 (in 9 @name mod4 @modulator 4))`; remove legacy mod5/mod6/ext1..ext4 declarations.",
            extra.join(", ")
        ));
    }

    if !mod_declared_params.is_empty() {
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
                "dsp.lisp declares host-modulatable parameter(s) with `@mod true` but is missing modulator input declaration(s): {}. Declare exactly the four configurable host modulation slots: `(def mod1 (in 6 @name mod1 @modulator 1))` through `(def mod4 (in 9 @name mod4 @modulator 4))`, when any param uses `@mod true`.",
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

    validate_forward_references(source, &exprs)?;

    let mut mod_declared_params = Vec::new();
    let mut mod_accessor_params = Vec::new();
    let mut direct_mod_refs = Vec::new();
    let mut declared_modulator_inputs = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    for expr in &exprs {
        if let Some(error) = effect_wrapper_form_error(expr) {
            return Err(error);
        }
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
        if let Some(input) = effect_input_decl(expr) {
            inputs.push(input);
        }
        if let Some(output) = effect_output_decl(expr) {
            outputs.push(output);
        }
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
        return Err(format!(
            "dsp.lisp uses `(mod param_name)` for effect parameter(s) not declared with `@mod true`: {}. Declare intentional effect modulation targets with `@mod true @mod-mode additive`, or read ordinary/internal controls directly by name.",
            locations.join(", ")
        ));
    }

    direct_mod_refs.sort();
    direct_mod_refs.dedup();
    if !direct_mod_refs.is_empty() {
        return Err(format!(
            "dsp.lisp directly reads host modulator input(s): {}. Declare modulator inputs only when params use @mod true, then read the host-modulated parameter with `(mod param_name)`; the effect mod matrix chooses which configured Mod slot drives that param.",
            direct_mod_refs.join(", ")
        ));
    }

    let required = ["mod1", "mod2", "mod3", "mod4"];
    let mut extra = declared_modulator_inputs
        .iter()
        .filter(|name| !required.iter().any(|required| *required == name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    extra.sort();
    extra.dedup();
    if !extra.is_empty() {
        return Err(format!(
            "dsp.lisp declares unsupported host modulator input(s): {}. Effects expose exactly four configurable host modulation slots after stereo audio inputs: `(def mod1 (in 3 @name mod1 @modulator 1))` through `(def mod4 (in 6 @name mod4 @modulator 4))`.",
            extra.join(", ")
        ));
    }

    if !mod_declared_params.is_empty() {
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
                "dsp.lisp declares host-modulatable effect parameter(s) with `@mod true` but is missing modulator input declaration(s): {}. Declare exactly the four configurable effect modulation slots: `(def mod1 (in 3 @name mod1 @modulator 1))` through `(def mod4 (in 6 @name mod4 @modulator 4))`, when any effect param uses `@mod true`.",
                missing.join(", ")
            ));
        }
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
                "@default" | "@min" | "@max" | "@unit" | "@group" | "@env" | "@role"
                    | "@mod" | "@mod-mode"
            )
    )
}

fn validate_forward_references(source: &str, exprs: &[Expression]) -> Result<(), String> {
    let mut declared_names = exprs
        .iter()
        .filter_map(top_level_binding_name)
        .collect::<Vec<_>>();
    declared_names.sort();
    declared_names.dedup();

    let mut available = std::collections::BTreeSet::new();
    let declared_names = declared_names
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for expr in exprs {
        if matches!(top_level_form_name(expr).as_deref(), Some("defmacro")) {
            if let Some(name) = top_level_def_name(expr) {
                available.insert(name);
            }
            continue;
        }

        let mut refs = Vec::new();
        collect_top_level_value_symbol_refs(expr, &mut refs);
        let mut missing = refs
            .into_iter()
            .filter(|name| declared_names.contains(name) && !available.contains(name))
            .collect::<Vec<_>>();
        missing.sort();
        missing.dedup();

        if !missing.is_empty() {
            let binding = top_level_binding_name(expr)
                .or_else(|| top_level_form_name(expr))
                .unwrap_or_else(|| "<top-level form>".to_string());
            let refs = missing
                .iter()
                .map(|name| match first_binding_line(source, name) {
                    Some(line) => format!("`{name}` defined later on line {line}"),
                    None => format!("`{name}` defined later"),
                })
                .collect::<Vec<_>>();
            let current_line = first_binding_line(source, &binding)
                .map(|line| format!(" on line {line}"))
                .unwrap_or_default();
            return Err(format!(
                "dsp.lisp references symbol(s) before they are defined: {}. The top-level form for `{binding}`{current_line} uses later binding(s). DGenLisp does not allow forward references; reorder the file so every `(def name ...)` appears before any top-level form that reads `name`.",
                refs.join(", ")
            ));
        }

        if let Some(name) = top_level_binding_name(expr) {
            available.insert(name);
        }
    }

    Ok(())
}

fn top_level_binding_name(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    match items.first() {
        Some(Expression::Symbol(head))
            if head == "def" || head == "defmacro" || head == "param" =>
        {
            match items.get(1) {
                Some(Expression::Symbol(name)) => Some(name.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn top_level_def_name(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "def" || head == "defmacro")
    {
        return None;
    }
    match items.get(1) {
        Some(Expression::Symbol(name)) => Some(name.clone()),
        _ => None,
    }
}

fn top_level_form_name(expr: &Expression) -> Option<String> {
    let Expression::List(items) = expr else {
        return None;
    };
    match items.first() {
        Some(Expression::Symbol(name)) => Some(name.clone()),
        _ => None,
    }
}

fn collect_value_symbol_refs(expr: &Expression, refs: &mut Vec<String>) {
    match expr {
        Expression::Symbol(symbol) => refs.push(symbol.clone()),
        Expression::List(items) => collect_list_value_symbol_refs(items, refs),
        Expression::Quasiquote(_) | Expression::QuoteList(_) | Expression::QuoteSymbol(_) => {}
        Expression::Unquote(expr) => collect_value_symbol_refs(expr, refs),
        Expression::Keyword(_) | Expression::String(_) | Expression::Number(_) => {}
    }
}

fn collect_top_level_value_symbol_refs(expr: &Expression, refs: &mut Vec<String>) {
    let Expression::List(items) = expr else {
        collect_value_symbol_refs(expr, refs);
        return;
    };
    match items.first() {
        Some(Expression::Symbol(head)) if head == "def" => {
            for item in items.iter().skip(2) {
                collect_value_symbol_refs(item, refs);
            }
        }
        Some(Expression::Symbol(head)) if head == "param" || head == "defmacro" => {}
        _ => collect_list_value_symbol_refs(items, refs),
    }
}

fn collect_list_value_symbol_refs(items: &[Expression], refs: &mut Vec<String>) {
    let mut index = 0usize;
    while index < items.len() {
        if matches!(&items[index], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            index += 2;
            continue;
        }
        collect_value_symbol_refs(&items[index], refs);
        index += 1;
    }
}

fn first_binding_line(source: &str, name: &str) -> Option<usize> {
    let def_needle = format!("(def {name}");
    let defmacro_needle = format!("(defmacro {name}");
    let param_needle = format!("(param {name}");
    source
        .lines()
        .position(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with(&def_needle)
                || trimmed.starts_with(&defmacro_needle)
                || trimmed.starts_with(&param_needle)
        })
        .map(|index| index + 1)
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
            (def clock (in 5 @name clock))
            (def mod1 (in 6 @name mod1 @modulator 1))
            (def mod2 (in 7 @name mod2 @modulator 2))
            (def mod3 (in 8 @name mod3 @modulator 3))
            (def mod4 (in 9 @name mod4 @modulator 4))
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
    fn effect_validator_accepts_host_modulation_forms() {
        validate_effect_dsp_source(
            r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (def mod1 (in 3 @name mod1 @modulator 1))
            (def mod2 (in 4 @name mod2 @modulator 2))
            (def mod3 (in 5 @name mod3 @modulator 3))
            (def mod4 (in 6 @name mod4 @modulator 4))
            (param depth @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
            (out (* in_l (mod depth)) 1 @name left)
            (out (* in_r (mod depth)) 2 @name right)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn effect_validator_rejects_direct_modulator_reads() {
        let err = validate_effect_dsp_source(
            r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (def mod1 (in 3 @name mod1 @modulator 1))
            (def mod2 (in 4 @name mod2 @modulator 2))
            (def mod3 (in 5 @name mod3 @modulator 3))
            (def mod4 (in 6 @name mod4 @modulator 4))
            (param depth @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
            (out (* in_l (mod depth)) 1 @name left)
            (out (* in_r mod1) 2 @name right)
            "#,
        )
        .unwrap_err();
        assert!(err.contains("directly reads host modulator input"));
        assert!(err.contains("mod1"));
    }

    #[test]
    fn effect_validator_rejects_modulatable_param_without_modulator_inputs() {
        let err = validate_effect_dsp_source(
            r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (param depth @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
            (out (* in_l (mod depth)) 1 @name left)
            (out (* in_r (mod depth)) 2 @name right)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("@mod true"));
        assert!(err.contains("missing modulator input"));
        assert!(err.contains("mod1"));
        assert!(err.contains("mod4"));
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
            (def clock (in 5 @name clock))
            (def mod1 (in 6 @name mod1 @modulator 1))
            (def mod2 (in 7 @name mod2 @modulator 2))
            (def mod3 (in 8 @name mod3 @modulator 3))
            (def mod4 (in 9 @name mod4 @modulator 4))
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
    fn rejects_forward_references_to_later_defs() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (param sustain_s @default 4.5 @min 0.5 @max 15.0)
            (def freq (* 440.0 (pow 2.0 (/ (- pitch 69.0) 12.0))))
            (def loop_cutoff (* freq 4.0))
            (def string1_out (svf string1 loop_cutoff 0.5 0))
            (def string1 (delay (+ trigger string1_out) 64.0))
            (out (* string1 gate velocity) 1 @name audio)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("references symbol(s) before they are defined"));
        assert!(err.contains("`string1` defined later"));
        assert!(err.contains("string1_out"));
        assert!(err.contains("DGenLisp does not allow forward references"));
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
        assert!(err.contains("mod4"));
    }

    #[test]
    fn rejects_legacy_extra_modulator_inputs() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def clock (in 5 @name clock))
            (def mod1 (in 6 @name mod1 @modulator 1))
            (def mod2 (in 7 @name mod2 @modulator 2))
            (def mod3 (in 8 @name mod3 @modulator 3))
            (def mod4 (in 9 @name mod4 @modulator 4))
            (def ext1 (in 11 @name ext1 @modulator 7))
            (param cutoff @default 900 @min 40 @max 12000 @mod true @mod-mode additive)
            (out (sin (* (phasor (mod cutoff)) twopi)) 1 @name audio)
            "#,
        )
        .unwrap_err();

        assert!(err.contains("unsupported host modulator input"));
        assert!(err.contains("ext1"));
        assert!(err.contains("exactly four"));
    }

    #[test]
    fn rejects_direct_modulator_refs_in_dsp_expressions() {
        let err = validate_instrument_dsp_source(
            r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def clock (in 5 @name clock))
            (def mod1 (in 6 @name mod1 @modulator 1))
            (def mod2 (in 7 @name mod2 @modulator 2))
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
