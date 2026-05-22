use eseqlisp::parser::{format_expression, ASTParser, Expression, Parser};
use std::collections::HashSet;

use super::network::AgentNetworkClient;
use super::providers::{
    default_model_presets, AgentMessage, AgentMessageRole, AgentProviderKind, AgentProviderState,
};

const MAX_RETRIES: usize = 1;

const DGENLISP_DEFMACRO_CONTRACT: &str = "\
DGenLisp defmacros are not Common Lisp macros. The macro body is ordinary \
DGenLisp DSP code evaluated directly by the host. Do not generate quoted code. \
Never use backquote, quote, unquote, let, lambda, or do. Use one or more local \
`(def name expr)` forms when intermediate values are needed, then end with the \
single result expression. For filters, use numeric modes such as `(svf input \
cutoff q 1)` for band-pass; do not use keyword modes such as `:bp`. Example: \
`(defmacro formant_bank (input f1 q1 g1 f2 q2 g2) (def b1 (svf input f1 q1 1)) \
(def b2 (svf input f2 q2 1)) (+ (* b1 g1) (* b2 g2)))`.";

#[derive(Debug, Clone)]
pub struct AgenticBubbleRequest {
    pub prompt: String,
    pub suggested_macro_name: String,
    pub follow_up: Option<AgenticBubbleFollowUp>,
}

#[derive(Debug, Clone)]
pub struct AgenticBubbleFollowUp {
    pub macro_name: String,
    pub params: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub enum AgenticBubbleOutput {
    Macro { macro_name: String, source: String },
    MacroEdit { source: String },
    Answer { text: String },
}

pub fn generate_agentic_bubble_macro(
    request: AgenticBubbleRequest,
) -> Result<AgenticBubbleOutput, String> {
    let provider_state = AgentProviderState::from_env();
    let provider = default_agentic_provider(&provider_state);
    let model = fast_model_for_provider(&provider_state, provider);
    eprintln!(
        "[agentic-bubble] start macro={} provider={provider:?} model={} prompt={:?}",
        request.suggested_macro_name, model, request.prompt
    );
    let client = AgentNetworkClient::load_default()?;
    let mut validation_error = None::<String>;
    let mut raw = String::new();
    for attempt in 0..=MAX_RETRIES {
        let system_prompt = system_prompt(request.follow_up.is_some());
        let prompt = user_prompt(&request, validation_error.as_deref());
        let messages = vec![AgentMessage {
            role: AgentMessageRole::User,
            content: prompt,
            reasoning_content: None,
            tool_name: None,
        }];
        eprintln!(
            "[agentic-bubble] request attempt={} macro={} model={}",
            attempt + 1,
            request.suggested_macro_name,
            model
        );
        raw = client
            .execute_text_turn(provider, &model, &system_prompt, &messages)
            .map_err(|error| {
                eprintln!(
                    "[agentic-bubble] request failed macro={} attempt={} error={}",
                    request.suggested_macro_name,
                    attempt + 1,
                    error.message
                );
                error.message
            })?;
        eprintln!(
            "[agentic-bubble] response macro={} attempt={} bytes={} raw={:?}",
            request.suggested_macro_name,
            attempt + 1,
            raw.len(),
            raw
        );
        match validate_agentic_response(&request, &raw) {
            Ok(output) => {
                eprintln!("[agentic-bubble] validated output={output:?}");
                return Ok(output);
            }
            Err(error) if attempt < MAX_RETRIES => {
                eprintln!(
                    "[agentic-bubble] validation failed macro={} attempt={} error={}",
                    request.suggested_macro_name,
                    attempt + 1,
                    error
                );
                validation_error = Some(error);
            }
            Err(error) => {
                eprintln!(
                    "[agentic-bubble] validation failed final macro={} error={} raw={:?}",
                    request.suggested_macro_name, error, raw
                );
                return Err(format!("{error}\n\nraw output:\n{raw}"));
            }
        }
    }
    Err(format!(
        "agentic bubble generation failed\n\nraw output:\n{raw}"
    ))
}

fn default_agentic_provider(state: &AgentProviderState) -> AgentProviderKind {
    if std::env::var(AgentProviderKind::Gemini.api_key_env())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        AgentProviderKind::Gemini
    } else {
        state.selected_provider
    }
}

fn fast_model_for_provider(state: &AgentProviderState, provider: AgentProviderKind) -> String {
    if let Ok(value) = std::env::var(provider.model_override_env()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if provider == AgentProviderKind::Gemini {
        return "gemini-3.5-flash".to_string();
    }
    default_model_presets()
        .into_iter()
        .find(|preset| preset.provider == provider && preset.id.contains("flash"))
        .or_else(|| {
            default_model_presets()
                .into_iter()
                .find(|preset| preset.provider == provider && preset.id.contains("mini"))
        })
        .or_else(|| {
            default_model_presets()
                .into_iter()
                .find(|preset| preset.provider == provider && preset.id.contains("nano"))
        })
        .map(|preset| preset.id)
        .or_else(|| state.selected_model().map(str::to_string))
        .unwrap_or_else(|| "gpt-5-mini".to_string())
}

fn system_prompt(follow_up: bool) -> String {
    let output_contract = if follow_up {
        "\
Output contract:
- If the user is asking a question or requesting an explanation, output exactly `(answer \"...\")`.
- If the user is requesting a change, output exactly one `(defmacro name (params...) body...)` form.
- For edits, preserve the provided macro name and parameter list exactly.
- Do not output prose, markdown fences, or any other wrapper."
    } else {
        "\
Output contract:
- Output exactly one `(defmacro name (params...) body...)` form.
- Do not output prose, explanations, markdown fences, or a complete instrument.
- Choose a short descriptive kebab-case macro name. Avoid generic prefixes such
  as `agentic`, `generated`, or `macro` unless they are genuinely part of the
  audio concept.
- Choose explicit parameters for every external signal/control the helper needs.
- Output one signal/value unless the user explicitly asks for a tuple-like result.
- Keep the macro small enough to be readable in a patcher node."
    };
    format!(
        "\
You generate small reusable DGenLisp `defmacro` helpers for an audio patcher.

{output_contract}

Macro syntax:
- {DGENLISP_DEFMACRO_CONTRACT}
- This is the same DSP dialect used by saved instrument `dsp.lisp` files, not the
  UI Lisp dialect and not Common Lisp.
- A macro body can contain multiple body forms. The last body form is the result.
- Use local `(def local_name expr)` forms for named intermediate signals.
- Local `(def ...)` and `make-history` names inside macros are scoped by the host.
- Do not use a top-level wrapper such as `(instrument ...)`, `(synth ...)`,
  `(definstrument ...)`, `(process ...)`, or `(main ...)`.
- DGenLisp is order-dependent inside macro bodies too: define a local before a
  later form reads it.

Valid DGenLisp macro examples:
`(defmacro formant_bank (input vowel_idx shift_st q_amt)
  (def shift_ratio (exp (/ (* (log 2) shift_st) 12)))
  (def v (+ (clip (round vowel_idx) 0 4) 1))
  (def f1 (clip (* shift_ratio (selector v 800 400 350 450 325)) 70 12000))
  (def f2 (clip (* shift_ratio (selector v 1150 1700 2200 800 700)) 90 13000))
  (def f3 (clip (* shift_ratio (selector v 2900 2600 3000 2830 2530)) 120 14000))
  (def q (clip q_amt 0.7 22.0))
  (def b1 (svf input f1 q 1))
  (def b2 (svf input f2 q 1))
  (def b3 (svf input f3 q 1))
  (tanh (* 1.45 (+ (* b1 1.00) (* b2 0.78) (* b3 0.52)))))`

`(defmacro smooth (sig amt)
  (make-history h)
  (def y (mix sig (read-history h) amt))
  (write-history h y))`

Invalid Common Lisp style. Never do this:
`(defmacro bad (input f q)
  `(let ((x ,input))
     (svf x ,f ,q :bp)))`

Available DGenLisp names from the bundled operator documentation:
{available_names}

Use only names from that documentation plus macro parameters and locals you define.
Do not invent operators such as `saw`, `pulse`, `dcblock`, or `sample-rate` unless
they appear in the available-name list above.",
        available_names = available_dgenlisp_names().join(", ")
    )
}

fn user_prompt(request: &AgenticBubbleRequest, validation_error: Option<&str>) -> String {
    let retry = validation_error
        .map(|error| format!("\nPrevious output was invalid: {error}\nReturn a corrected macro."))
        .unwrap_or_default();
    if let Some(follow_up) = &request.follow_up {
        return format!(
            "Follow-up prompt: {}\n\nSelected macro name: {}\nSelected macro params: ({})\nSelected macro source:\n{}\n\nDecide whether the user wants an explanation/answer or a macro edit. For an answer, return `(answer \"...\")`. For an edit, return a complete `(defmacro {} ({}) body...)` preserving the exact name and params.\n{}{}",
            request.prompt.trim(),
            follow_up.macro_name,
            follow_up.params.join(" "),
            follow_up.source,
            follow_up.macro_name,
            follow_up.params.join(" "),
            DGENLISP_DEFMACRO_CONTRACT,
            retry
        );
    }
    format!(
        "Prompt: {}\nProduce a defmacro. Choose the macro name yourself. Suggested fallback name if needed: {}. Choose a clear parameter list. Output a single signal unless the prompt explicitly requires otherwise.\n{}{}",
        request.prompt.trim(),
        request.suggested_macro_name,
        DGENLISP_DEFMACRO_CONTRACT,
        retry
    )
}

fn validate_agentic_response(
    request: &AgenticBubbleRequest,
    raw: &str,
) -> Result<AgenticBubbleOutput, String> {
    if let Some(follow_up) = &request.follow_up {
        validate_follow_up_response(raw, follow_up)
    } else {
        let (macro_name, source) = validate_macro_response(raw)?;
        Ok(AgenticBubbleOutput::Macro { macro_name, source })
    }
}

fn validate_follow_up_response(
    raw: &str,
    follow_up: &AgenticBubbleFollowUp,
) -> Result<AgenticBubbleOutput, String> {
    let source = extract_lisp_source(raw);
    let tokens = Parser::new(source.clone())
        .parse()
        .map_err(|error| format!("parser token error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("parser AST error: {error:?}"))?;
    if exprs.len() != 1 {
        return Err(format!(
            "expected exactly one top-level form, got {}",
            exprs.len()
        ));
    }
    let Expression::List(items) = &exprs[0] else {
        return Err("top-level form must be a list".to_string());
    };
    match items.as_slice() {
        [Expression::Symbol(head), Expression::String(text)] if head == "answer" => {
            let text = text.trim();
            if text.is_empty() {
                return Err("answer text cannot be empty".to_string());
            }
            Ok(AgenticBubbleOutput::Answer {
                text: text.to_string(),
            })
        }
        [Expression::Symbol(head), Expression::Symbol(name), Expression::List(params), body @ ..]
            if head == "defmacro" =>
        {
            if name != &follow_up.macro_name {
                return Err(format!(
                    "edited macro name `{name}` must remain `{}`",
                    follow_up.macro_name
                ));
            }
            let actual_params = params
                .iter()
                .map(|param| match param {
                    Expression::Symbol(param) => Ok(param.clone()),
                    _ => Err("defmacro parameters must be symbols".to_string()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if actual_params != follow_up.params {
                return Err(format!(
                    "edited macro params `({})` must remain `({})`",
                    actual_params.join(" "),
                    follow_up.params.join(" ")
                ));
            }
            if body.is_empty() {
                return Err("defmacro body is empty".to_string());
            }
            validate_known_operators(body, params)?;
            Ok(AgenticBubbleOutput::MacroEdit {
                source: format_expression(&exprs[0]),
            })
        }
        _ => Err("response must be `(answer \"...\")` or the edited defmacro".to_string()),
    }
}

fn validate_macro_response(raw: &str) -> Result<(String, String), String> {
    let source = extract_lisp_source(raw);
    let tokens = Parser::new(source.clone())
        .parse()
        .map_err(|error| format!("parser token error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("parser AST error: {error:?}"))?;
    if exprs.len() != 1 {
        return Err(format!(
            "expected exactly one top-level form, got {}",
            exprs.len()
        ));
    }
    let Expression::List(items) = &exprs[0] else {
        return Err("top-level form must be a list".to_string());
    };
    match items.as_slice() {
        [Expression::Symbol(head), Expression::Symbol(name), Expression::List(params), body @ ..]
            if head == "defmacro" =>
        {
            if !valid_symbol(name) {
                return Err(format!("invalid defmacro name {name}"));
            }
            if body.is_empty() {
                return Err("defmacro body is empty".to_string());
            }
            for param in params {
                let Expression::Symbol(param) = param else {
                    return Err("defmacro parameters must be symbols".to_string());
                };
                if !valid_symbol(param) {
                    return Err(format!("invalid parameter symbol {param}"));
                }
            }
            validate_known_operators(body, params)?;
            Ok((name.clone(), format_expression(&exprs[0])))
        }
        _ => Err("response must be exactly one defmacro".to_string()),
    }
}

fn extract_lisp_source(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after.strip_prefix("lisp").unwrap_or(after);
        let after = after.strip_prefix('\n').unwrap_or(after);
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn validate_known_operators(body: &[Expression], params: &[Expression]) -> Result<(), String> {
    let mut allowed = dgen_operator_names();
    for param in params {
        if let Expression::Symbol(name) = param {
            allowed.insert(name.clone());
        }
    }
    for expr in body {
        validate_body_expr(expr, &mut allowed)?;
    }
    Ok(())
}

fn validate_body_expr(expr: &Expression, allowed: &mut HashSet<String>) -> Result<(), String> {
    let Expression::List(items) = expr else {
        return validate_expr(expr, allowed);
    };
    match items.as_slice() {
        [Expression::Symbol(head), binding, value @ ..] if head == "def" => {
            if value.is_empty() {
                return Err("local def must include a value expression".to_string());
            }
            for expr in value {
                validate_expr(expr, allowed)?;
            }
            insert_local_binding(binding, allowed)
        }
        [Expression::Symbol(head), Expression::Symbol(name)] if head == "make-history" => {
            if !valid_symbol(name) {
                return Err(format!("invalid history symbol {name}"));
            }
            allowed.insert(name.clone());
            Ok(())
        }
        [Expression::Symbol(head), ..] if head == "make-history" => {
            Err("make-history must be `(make-history name)`".to_string())
        }
        _ => validate_expr(expr, allowed),
    }
}

fn insert_local_binding(binding: &Expression, allowed: &mut HashSet<String>) -> Result<(), String> {
    match binding {
        Expression::Symbol(name) => {
            if !valid_symbol(name) {
                return Err(format!("invalid local def symbol {name}"));
            }
            allowed.insert(name.clone());
            Ok(())
        }
        Expression::List(names) => {
            for name in names {
                let Expression::Symbol(name) = name else {
                    return Err("tuple local def bindings must be symbols".to_string());
                };
                if !valid_symbol(name) {
                    return Err(format!("invalid tuple local def symbol {name}"));
                }
                allowed.insert(name.clone());
            }
            Ok(())
        }
        _ => Err("local def binding must be a symbol or tuple of symbols".to_string()),
    }
}

fn validate_expr(expr: &Expression, allowed: &HashSet<String>) -> Result<(), String> {
    match expr {
        Expression::List(items) => {
            if let Some(Expression::Symbol(head)) = items.first() {
                if !allowed.contains(head) && !valid_number_symbol(head) {
                    return Err(format!("unknown operator or symbol {head}"));
                }
            }
            for item in items {
                validate_expr(item, allowed)?;
            }
            Ok(())
        }
        Expression::Symbol(symbol) if !allowed.contains(symbol) && !valid_number_symbol(symbol) => {
            Err(format!("unknown symbol {symbol}"))
        }
        Expression::Quasiquote(_)
        | Expression::Unquote(_)
        | Expression::QuoteList(_)
        | Expression::QuoteSymbol(_) => Err("quoted generated code is not allowed".to_string()),
        Expression::Keyword(keyword) => Err(format!(
            "keyword arguments are not allowed in generated DSP macros: :{keyword}"
        )),
        _ => Ok(()),
    }
}

fn valid_symbol(symbol: &str) -> bool {
    !symbol.is_empty()
        && !symbol.chars().any(char::is_whitespace)
        && !symbol
            .chars()
            .any(|ch| matches!(ch, '(' | ')' | '"' | '\'' | '`' | ';'))
}

fn valid_number_symbol(symbol: &str) -> bool {
    symbol.parse::<f64>().is_ok()
}

fn dgen_operator_names() -> HashSet<String> {
    available_dgenlisp_names().into_iter().collect()
}

fn available_dgenlisp_names() -> Vec<String> {
    let metadata: serde_json::Value =
        serde_json::from_str(include_str!("../../tools/dgenlisp-operators.json"))
            .expect("bundled dgenlisp-operators.json must be valid JSON");
    let mut names = Vec::new();
    if let Some(operators) = metadata
        .get("operators")
        .and_then(serde_json::Value::as_array)
    {
        for operator in operators {
            if let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) {
                names.push(name.to_string());
            }
            if let Some(aliases) = operator
                .get("aliases")
                .and_then(serde_json::Value::as_array)
            {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        names.push(alias.to_string());
                    }
                }
            }
        }
    }
    if let Some(special_forms) = metadata
        .get("special_forms")
        .and_then(serde_json::Value::as_array)
    {
        for special_form in special_forms {
            if let Some(name) = special_form.get("name").and_then(serde_json::Value::as_str) {
                if name != "defmacro" {
                    names.push(name.to_string());
                }
            }
        }
    }
    if let Some(constants) = metadata
        .get("constants")
        .and_then(serde_json::Value::as_array)
    {
        for constant in constants {
            if let Some(name) = constant.get("name").and_then(serde_json::Value::as_str) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::{
        system_prompt, user_prompt, validate_follow_up_response, validate_macro_response,
        AgenticBubbleFollowUp, AgenticBubbleOutput, AgenticBubbleRequest,
    };

    #[test]
    fn validates_single_named_defmacro() {
        let source = "(defmacro agentic-gain (x amount) (* x amount))";
        let (name, validated) = validate_macro_response(source).expect("valid macro");
        assert_eq!(name, "agentic-gain");
        assert!(validated.contains("defmacro agentic-gain"));
    }

    #[test]
    fn validates_sequential_local_defs() {
        let source = "\
(defmacro agentic-formants (input f1 q1 g1 f2 q2 g2 f3 q3 g3)
  (def filter1 (svf input f1 q1 1))
  (def filter2 (svf input f2 q2 1))
  (def filter3 (svf input f3 q3 1))
  (+ (* filter1 g1) (+ (* filter2 g2) (* filter3 g3))))";
        let (name, validated) = validate_macro_response(source).expect("valid macro");
        assert_eq!(name, "agentic-formants");
        assert!(validated.contains("filter1"));
    }

    #[test]
    fn validates_history_binding_scope() {
        let source = "\
(defmacro agentic-smooth (sig amt)
  (make-history h)
  (def y (mix sig (read-history h) amt))
  (write-history h y))";
        let (name, validated) = validate_macro_response(source).expect("valid macro");
        assert_eq!(name, "agentic-smooth");
        assert!(validated.contains("make-history h"));
    }

    #[test]
    fn accepts_model_chosen_macro_name() {
        let (name, _) = validate_macro_response("(defmacro other (x) x)").expect("valid macro");
        assert_eq!(name, "other");
    }

    #[test]
    fn prompt_describes_dgenlisp_macro_body_shape() {
        let request = AgenticBubbleRequest {
            prompt: "create a formant bank".to_string(),
            suggested_macro_name: "agentic-formants".to_string(),
            follow_up: None,
        };
        let system = system_prompt(false);
        let user = user_prompt(&request, None);
        for prompt in [system.as_str(), user.as_str()] {
            assert!(prompt.contains("not Common Lisp macros"));
            assert!(prompt.contains("Never use backquote"));
            assert!(prompt.contains("`(def name expr)`"));
            assert!(prompt.contains("(svf input cutoff q 1)"));
        }
        assert!(system.contains("Available DGenLisp names from the bundled operator documentation"));
        assert!(system.contains("polyblep_saw"));
        assert!(!system.contains("such as +, -, *, /, sin"));
        assert!(!system.contains("Use only common DGenLisp primitives"));
    }

    #[test]
    fn follow_up_accepts_answer_envelope() {
        let follow_up = AgenticBubbleFollowUp {
            macro_name: "smooth".to_string(),
            params: vec!["sig".to_string(), "amt".to_string()],
            source: "(defmacro smooth (sig amt) (mix sig amt 0.5))".to_string(),
        };
        let output = validate_follow_up_response("(answer \"It smooths a signal.\")", &follow_up)
            .expect("valid answer");
        assert!(matches!(output, AgenticBubbleOutput::Answer { text } if text.contains("smooths")));
    }

    #[test]
    fn follow_up_rejects_signature_changes() {
        let follow_up = AgenticBubbleFollowUp {
            macro_name: "smooth".to_string(),
            params: vec!["sig".to_string(), "amt".to_string()],
            source: "(defmacro smooth (sig amt) (mix sig amt 0.5))".to_string(),
        };
        let renamed = validate_follow_up_response(
            "(defmacro smoother (sig amt) (mix sig amt 0.5))",
            &follow_up,
        )
        .unwrap_err();
        assert!(renamed.contains("must remain `smooth`"));
        let reparam = validate_follow_up_response(
            "(defmacro smooth (sig amount) (mix sig amount 0.5))",
            &follow_up,
        )
        .unwrap_err();
        assert!(reparam.contains("must remain `(sig amt)`"));
    }

    #[test]
    fn rejects_common_lisp_macro_syntax() {
        let error = validate_macro_response(
            "(defmacro agentic-formants (input f q) `(svf ,input ,f ,q :bp))",
        )
        .unwrap_err();
        assert!(error.contains("quoted generated code"));
    }

    #[test]
    fn rejects_let_body() {
        let error = validate_macro_response(
            "(defmacro agentic-formants (input f q) (let ((x input)) (svf x f q 1)))",
        )
        .unwrap_err();
        assert!(error.contains("unknown operator or symbol let"));
    }

    #[test]
    fn rejects_keyword_filter_modes() {
        let error =
            validate_macro_response("(defmacro agentic-formants (input f q) (svf input f q :bp))")
                .unwrap_err();
        assert!(error.contains("keyword arguments are not allowed"));
    }

    #[test]
    fn rejects_forward_reference_to_later_local_def() {
        let error = validate_macro_response(
            "(defmacro agentic-formants (input f q) (+ filtered input) (def filtered (svf input f q 1)))"
        )
        .unwrap_err();
        assert!(error.contains("unknown symbol filtered"));
    }
}
