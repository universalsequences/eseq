use eseqlisp::parser::{format_expression, ASTParser, Expression, Parser};
use eseqlisp::widget_render::patcher::PatcherConnectOp;
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
result expression. Return a single signal by default; when the request naturally \
needs multiple patcher outlets, make the final expression `(tuple out1 out2 ...)`. \
For filters, use numeric modes such as `(svf input \
cutoff q 1)` for band-pass; do not use keyword modes such as `:bp`. Example: \
`(defmacro formant_bank (input f1 q1 g1 f2 q2 g2) (def b1 (svf input f1 q1 1)) \
(def b2 (svf input f2 q2 1)) (+ (* b1 g1) (* b2 g2)))`.";

#[derive(Debug, Clone)]
pub struct AgenticBubbleRequest {
    pub prompt: String,
    pub suggested_macro_name: String,
    pub follow_up: Option<AgenticBubbleFollowUp>,
    pub connect: Option<AgenticBubbleConnect>,
    /// Earlier turns of this bubble's conversation, oldest first, as
    /// `(question, answer)`. Replayed ahead of the current prompt so a
    /// follow-up like "why?" resolves against what was already said.
    pub history: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct AgenticBubbleFollowUp {
    pub macro_name: String,
    pub params: Vec<String>,
    pub source: String,
}

/// A connect bubble's payload: the node to wire and the surrounding patch, as
/// assembled by the patcher (docs/patcher-agentic-connect-spec.md §5).
#[derive(Debug, Clone)]
pub struct AgenticBubbleConnect {
    pub subject_node_id: String,
    pub context: String,
}

#[derive(Debug, Clone)]
pub enum AgenticBubbleOutput {
    Macro { macro_name: String, source: String },
    MacroEdit { source: String },
    Answer { text: String },
    Connections { ops: Vec<PatcherConnectOp> },
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
        // A connect bubble shares no prompt text with the create and edit
        // bubbles, so neither can bleed context into the other (spec §3).
        let system_prompt = match &request.connect {
            Some(_) => connect_system_prompt(),
            None => system_prompt(request.follow_up.is_some()),
        };
        let prompt = match &request.connect {
            Some(connect) => connect_user_prompt(&request, connect, validation_error.as_deref()),
            None => user_prompt(&request, validation_error.as_deref()),
        };
        // Earlier turns go in verbatim; only the last user message carries the
        // macro context and output contract, so a retry still restates them.
        let mut messages = Vec::with_capacity(request.history.len() * 2 + 1);
        for (question, answer) in &request.history {
            messages.push(AgentMessage {
                role: AgentMessageRole::User,
                content: question.clone(),
                reasoning_content: None,
                tool_name: None,
            });
            messages.push(AgentMessage {
                role: AgentMessageRole::Assistant,
                content: answer.clone(),
                reasoning_content: None,
                tool_name: None,
            });
        }
        messages.push(AgentMessage {
            role: AgentMessageRole::User,
            content: prompt,
            reasoning_content: None,
            tool_name: None,
        });
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
    // An explicit `M-x choose-model` pick outranks the built-in preference.
    if let Some(provider) = super::model_choice::agentic_provider() {
        return provider;
    }
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
    // A chosen model wins outright, but only for the provider it belongs to —
    // if some other caller forced a different provider, fall through to that
    // provider's own fast default rather than sending it a foreign model id.
    if let Some(chosen) = super::model_choice::agentic_model() {
        if super::model_choice::agentic_provider() == Some(provider) {
            return chosen;
        }
    }
    if let Ok(value) = std::env::var(provider.model_override_env()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if provider == AgentProviderKind::Gemini {
        return "gemini-3.5-flash".to_string();
    }
    // Anthropic ids carry no "flash"/"mini"/"nano" marker for the scan below
    // to find, so name the fast tier outright.
    if provider == AgentProviderKind::Anthropic {
        return "claude-haiku-4-5".to_string();
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
- Output one signal/value by default. If the prompt asks for multiple results,
  parallel variants, stereo left/right, dry/wet taps, analysis plus processed
  signal, phase plus waveform, or otherwise implies several patcher outlets,
  return `(tuple out1 out2 ...)` as the final body form.
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
- If the last body form is `(tuple a b c)`, the patcher exposes one outlet for
  each tuple item. Do not wrap a single output in `tuple`.
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

`(defmacro split-drive (input drive mix_amt)
  (def dry input)
  (def wet (tanh (* input drive)))
  (def blended (mix dry wet mix_amt))
  (tuple blended wet))`

Invalid Common Lisp style. Never do this:
`(defmacro bad (input f q)
  `(let ((x ,input))
     (svf x ,f ,q :bp)))`

Available DGenLisp operators from the bundled operator documentation. Each line is
the exact call signature. Argument counts are enforced: pass exactly the arguments
a signature lists, no more. A `...` in a signature means the operator is variadic;
without it the argument count is fixed. In particular `delay` takes exactly two
arguments and has no max-delay-time argument, unlike Max/gen~.
{available_names}

Use only names from that documentation plus macro parameters and locals you define.
Do not invent operators such as `saw`, `pulse`, `dcblock`, or `sample-rate` unless
they appear in the signature list above.
Do not use `@attribute` arguments; pass positional arguments only.",
        available_names = dgen_reference()
    )
}

fn user_prompt(request: &AgenticBubbleRequest, validation_error: Option<&str>) -> String {
    let retry = validation_error
        .map(|error| format!("\nPrevious output was invalid: {error}\nReturn a corrected macro."))
        .unwrap_or_default();
    let continuing = if request.history.is_empty() {
        ""
    } else {
        "This continues the conversation above about the same macro; resolve pronouns and \
         shorthand against those earlier turns.\n\n"
    };
    if let Some(follow_up) = &request.follow_up {
        return format!(
            "{continuing}Follow-up prompt: {}\n\nSelected macro name: {}\nSelected macro params: ({})\nSelected macro source:\n{}\n\nDecide whether the user wants an explanation/answer or a macro edit. For an answer, return `(answer \"...\")`. For an edit, return a complete `(defmacro {} ({}) body...)` preserving the exact name and params.\n{}{}",
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
        "Prompt: {}\nProduce a defmacro. Choose the macro name yourself. Suggested fallback name if needed: {}. Choose a clear parameter list. Output one signal by default, but use a final `(tuple ...)` when the prompt naturally needs multiple patcher outlets.\n{}{}",
        request.prompt.trim(),
        request.suggested_macro_name,
        DGENLISP_DEFMACRO_CONTRACT,
        retry
    )
}

fn connect_system_prompt() -> String {
    "\
You wire an existing node into an existing audio patch. You never write code.

Output contract:
- Output exactly one JSON object: {\"ops\": [ ... ]}.
- No prose, no explanation, no markdown fences.
- Each op is either
  {\"op\": \"connect\", \"from_node\": \"<id>\", \"from_outlet\": <int>,
   \"to_node\": \"<id>\", \"to_arg\": <int>, \"why\": \"<short reason>\"}
  or
  {\"op\": \"inline\", \"value\": <number>, \"to_node\": \"<id>\",
   \"to_arg\": <int>, \"why\": \"<short reason>\"}.
- Use node ids exactly as given. Labels are decoration and are not accepted.
- Address inlets by argument index (the `in <index>` lines), never by drawn port
  position.
- Only target inlets the context marks `free`. An inlet that is cabled, holds a
  literal, or holds an inline param already has a value; leave it alone.
- Prefer `inline` for constants: a number belongs inside the node, not on the
  end of a cable. Use `connect` when a signal should flow.
- Emit no op at all for an inlet you have no good reason to fill. A short
  correct plan beats a complete one.
- An empty plan is {\"ops\": []}."
        .to_string()
}

fn connect_user_prompt(
    request: &AgenticBubbleRequest,
    connect: &AgenticBubbleConnect,
    validation_error: Option<&str>,
) -> String {
    let retry = validation_error
        .map(|error| format!("\nPrevious output was invalid: {error}\nReturn a corrected plan."))
        .unwrap_or_default();
    format!(
        "Prompt: {}\n\nWire node {} into this patch.\n\n{}{}",
        request.prompt.trim(),
        connect.subject_node_id,
        connect.context,
        retry
    )
}

fn validate_connect_response(raw: &str) -> Result<AgenticBubbleOutput, String> {
    let source = extract_json_source(raw);
    let value: serde_json::Value = serde_json::from_str(&source)
        .map_err(|error| format!("connection plan is not valid JSON: {error}"))?;
    let items = value
        .get("ops")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "connection plan must be {\"ops\": [...]}".to_string())?;
    let ops = items
        .iter()
        .map(parse_connect_op)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AgenticBubbleOutput::Connections { ops })
}

fn parse_connect_op(value: &serde_json::Value) -> Result<PatcherConnectOp, String> {
    let field = |key: &str| -> Result<String, String> {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("op is missing string field `{key}`"))
    };
    let index = |key: &str| -> Result<usize, String> {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|index| index as usize)
            .ok_or_else(|| format!("op is missing integer field `{key}`"))
    };
    let why = value
        .get("why")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    match field("op")?.as_str() {
        "connect" => Ok(PatcherConnectOp::Connect {
            from_node: field("from_node")?,
            from_outlet: index("from_outlet")?,
            to_node: field("to_node")?,
            to_arg: index("to_arg")?,
            why,
        }),
        "inline" => Ok(PatcherConnectOp::Inline {
            // Numbers are accepted unquoted; the host canonicalizes the literal.
            value: value
                .get("value")
                .and_then(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| value.as_f64().map(|value| value.to_string()))
                })
                .ok_or_else(|| "inline op is missing `value`".to_string())?,
            to_node: field("to_node")?,
            to_arg: index("to_arg")?,
            why,
        }),
        other => Err(format!("unknown op `{other}`")),
    }
}

fn extract_json_source(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        let after = after.strip_prefix('\n').unwrap_or(after);
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn validate_agentic_response(
    request: &AgenticBubbleRequest,
    raw: &str,
) -> Result<AgenticBubbleOutput, String> {
    if request.connect.is_some() {
        return validate_connect_response(raw);
    }
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
    let arities = dgen_operator_arities();
    allowed.insert("tuple".to_string());
    for param in params {
        if let Expression::Symbol(name) = param {
            allowed.insert(name.clone());
        }
    }
    for expr in body {
        validate_body_expr(expr, &mut allowed, &arities)?;
    }
    Ok(())
}

fn validate_body_expr(
    expr: &Expression,
    allowed: &mut HashSet<String>,
    arities: &std::collections::HashMap<String, OperatorArity>,
) -> Result<(), String> {
    let Expression::List(items) = expr else {
        return validate_expr(expr, allowed, arities);
    };
    match items.as_slice() {
        [Expression::Symbol(head), binding, value @ ..] if head == "def" => {
            if value.is_empty() {
                return Err("local def must include a value expression".to_string());
            }
            for expr in value {
                validate_expr(expr, allowed, arities)?;
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
        _ => validate_expr(expr, allowed, arities),
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

fn validate_expr(
    expr: &Expression,
    allowed: &HashSet<String>,
    arities: &std::collections::HashMap<String, OperatorArity>,
) -> Result<(), String> {
    match expr {
        Expression::List(items) => {
            if let Some(Expression::Symbol(head)) = items.first() {
                if !allowed.contains(head) && !valid_number_symbol(head) {
                    return Err(format!("unknown operator or symbol {head}"));
                }
                check_arity(head, items, arities)?;
            }
            for item in items {
                validate_expr(item, allowed, arities)?;
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

fn dgen_metadata() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tools/dgenlisp-operators.json"))
        .expect("bundled dgenlisp-operators.json must be valid JSON")
}

/// One call signature per operator, for the system prompt. The bundled docs carry
/// arity and port names for every operator; a bare name list leaves the model to
/// guess argument counts from other DSP dialects (Max/gen~ `delay` takes a third
/// max-delay-time argument, ours does not).
fn dgen_reference() -> String {
    let metadata = dgen_metadata();
    let mut lines = Vec::new();
    if let Some(operators) = metadata
        .get("operators")
        .and_then(serde_json::Value::as_array)
    {
        for operator in operators {
            let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let mut line = operator_signature_text(name, operator);
            let aliases = json_str_array(operator.get("aliases"));
            if !aliases.is_empty() {
                line.push_str(&format!("   [also: {}]", aliases.join(", ")));
            }
            lines.push(line);
        }
    }
    for key in ["special_forms", "constants"] {
        if let Some(entries) = metadata.get(key).and_then(serde_json::Value::as_array) {
            for entry in entries {
                let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if name == "defmacro" {
                    continue;
                }
                if key == "constants" {
                    lines.push(name.to_string());
                } else {
                    lines.push(operator_signature_text(name, entry));
                }
            }
        }
    }
    lines.sort();
    lines.dedup();
    lines.join("\n")
}

/// Prefer the curated signatures, skipping `@attribute` forms the validator rejects
/// anyway. Falls back to the documented port names when every signature is
/// attribute-only.
fn operator_signature_text(name: &str, entry: &serde_json::Value) -> String {
    let signatures: Vec<String> = json_str_array(entry.get("signatures"))
        .into_iter()
        .filter(|signature| !signature.contains('@'))
        .collect();
    if !signatures.is_empty() {
        return signatures.join(" | ");
    }
    let mut parts = vec![name.to_string()];
    if let Some(inputs) = entry.get("inputs").and_then(serde_json::Value::as_array) {
        for input in inputs {
            if let Some(input) = input.get("name").and_then(serde_json::Value::as_str) {
                parts.push(input.to_string());
            }
        }
    }
    let (_, maximum, _) = arity_bounds(entry);
    if maximum.map_or(true, |maximum| maximum + 1 > parts.len()) {
        parts.push("...".to_string());
    }
    format!("({})", parts.join(" "))
}

#[derive(Debug, Clone)]
struct OperatorArity {
    minimum: Option<usize>,
    maximum: Option<usize>,
    signature: String,
}

fn arity_bounds(entry: &serde_json::Value) -> (Option<usize>, Option<usize>, bool) {
    let Some(arity) = entry.get("arity") else {
        return (None, None, false);
    };
    let read = |key: &str| {
        arity
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
    };
    // `+`, `*`, `min`, ... document a binary arity but the parser folds n-ary calls
    // down to nested binary ones, so they accept any number of arguments.
    let nary = arity
        .get("parser_rewrites_nary")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (read("minimum"), read("maximum"), nary)
}

/// Argument-count bounds per operator name (and alias). Special forms are absent:
/// `def`, `make-history` and friends are shape-checked separately.
fn dgen_operator_arities() -> std::collections::HashMap<String, OperatorArity> {
    let metadata = dgen_metadata();
    let mut arities = std::collections::HashMap::new();
    let Some(operators) = metadata
        .get("operators")
        .and_then(serde_json::Value::as_array)
    else {
        return arities;
    };
    for operator in operators {
        let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let (minimum, maximum, nary) = arity_bounds(operator);
        if minimum.is_none() && (maximum.is_none() || nary) {
            continue;
        }
        let arity = OperatorArity {
            minimum,
            maximum: if nary { None } else { maximum },
            signature: operator_signature_text(name, operator),
        };
        for name in std::iter::once(name.to_string()).chain(json_str_array(operator.get("aliases")))
        {
            arities.insert(name, arity.clone());
        }
    }
    arities
}

/// Arguments excluding `@attribute value` pairs, which are not positional.
fn positional_arg_count(items: &[Expression]) -> usize {
    let mut count = 0;
    let mut index = 1;
    while index < items.len() {
        if matches!(&items[index], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            index += 2;
            continue;
        }
        count += 1;
        index += 1;
    }
    count
}

fn check_arity(
    head: &str,
    items: &[Expression],
    arities: &std::collections::HashMap<String, OperatorArity>,
) -> Result<(), String> {
    let Some(arity) = arities.get(head) else {
        return Ok(());
    };
    let count = positional_arg_count(items);
    if let Some(minimum) = arity.minimum {
        if count < minimum {
            return Err(format!(
                "operator {head} takes at least {minimum} argument(s) but got {count}: {}",
                arity.signature
            ));
        }
    }
    if let Some(maximum) = arity.maximum {
        if count > maximum {
            return Err(format!(
                "operator {head} takes at most {maximum} argument(s) but got {count}: {}",
                arity.signature
            ));
        }
    }
    Ok(())
}

fn json_str_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
        connect_system_prompt, connect_user_prompt, dgen_reference, system_prompt, user_prompt,
        validate_connect_response, validate_follow_up_response, validate_macro_response,
        AgenticBubbleConnect, AgenticBubbleFollowUp, AgenticBubbleOutput, AgenticBubbleRequest,
        PatcherConnectOp,
    };

    #[test]
    fn reference_lists_call_signatures_not_bare_names() {
        let reference = dgen_reference();
        assert!(
            reference.contains("(delay signal time_in_samples)"),
            "reference must carry the delay signature: {reference}"
        );
        assert!(reference.contains("(svf input cutoff q mode)"));
        // Attribute-only signatures are replaced by a positional form the validator accepts.
        assert!(!reference.contains('@'), "reference leaks attribute forms");
    }

    #[test]
    fn system_prompt_carries_signatures() {
        let prompt = system_prompt(false);
        assert!(prompt.contains("(delay signal time_in_samples)"));
        assert!(prompt.contains("Argument counts are enforced"));
    }

    #[test]
    fn rejects_max_msp_style_third_delay_argument() {
        // Max/gen~ spells this `(delay signal time maxtime)`; ours has no third inlet.
        let source = "(defmacro echo (input time) (delay input time 44100))";
        let error = validate_macro_response(source).expect_err("three-argument delay is invalid");
        assert!(error.contains("delay"), "{error}");
        assert!(error.contains("at most 2"), "{error}");
    }

    #[test]
    fn rejects_too_few_arguments() {
        let source = "(defmacro echo (input) (delay input))";
        let error = validate_macro_response(source).expect_err("one-argument delay is invalid");
        assert!(error.contains("at least 2"), "{error}");
    }

    #[test]
    fn accepts_nary_calls_for_parser_folded_operators() {
        // `+` documents a binary arity but the parser folds n-ary calls.
        let source = "(defmacro sum3 (a b c) (+ a b c))";
        validate_macro_response(source).expect("n-ary + is valid");
    }

    #[test]
    fn accepts_documented_arities() {
        let source = "\
(defmacro band (input f q amount)
  (def filtered (svf input f q 1))
  (def shaped (mix input filtered amount))
  (clip shaped -1 1))";
        validate_macro_response(source).expect("documented arities are valid");
    }

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
    fn validates_tuple_return_for_multi_output_macro() {
        let source = "\
(defmacro split-drive (input drive mix_amt)
  (def dry input)
  (def wet (tanh (* input drive)))
  (def blended (mix dry wet mix_amt))
  (tuple blended wet))";
        let (name, validated) = validate_macro_response(source).expect("valid macro");
        assert_eq!(name, "split-drive");
        assert!(validated.contains("(tuple blended wet)"));
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
            connect: None,
            history: Vec::new(),
        };
        let system = system_prompt(false);
        let user = user_prompt(&request, None);
        for prompt in [system.as_str(), user.as_str()] {
            assert!(prompt.contains("not Common Lisp macros"));
            assert!(prompt.contains("Never use backquote"));
            assert!(prompt.contains("`(def name expr)`"));
            assert!(prompt.contains("(svf input cutoff q 1)"));
        }
        assert!(
            system.contains("Available DGenLisp operators from the bundled operator documentation")
        );
        assert!(system.contains("final expression `(tuple out1 out2 ...)`"));
        assert!(system.contains("one outlet for"));
        assert!(user.contains("multiple patcher outlets"));
        assert!(system.contains("polyblep_saw"));
        assert!(!system.contains("such as +, -, *, /, sin"));
        assert!(!system.contains("Use only common DGenLisp primitives"));
    }

    #[test]
    fn prompt_recommends_tuple_when_request_implies_multiple_outputs() {
        let request = AgenticBubbleRequest {
            prompt: "make a helper that returns dry, wet, and mixed outputs".to_string(),
            suggested_macro_name: "split-drive".to_string(),
            follow_up: None,
            connect: None,
            history: Vec::new(),
        };
        let user = user_prompt(&request, None);
        assert!(user.contains("use a final `(tuple ...)`"));
        assert!(user.contains("multiple patcher outlets"));
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

    #[test]
    fn connect_plan_parses_both_op_kinds() {
        let raw = "```json\n{\"ops\": [\
{\"op\": \"connect\", \"from_node\": \"trig-in\", \"from_outlet\": 0, \
 \"to_node\": \"created-3\", \"to_arg\": 0, \"why\": \"trigger\"}, \
{\"op\": \"inline\", \"value\": 0.8, \"to_node\": \"created-3\", \
 \"to_arg\": 2, \"why\": \"decay needs a value\"}]}\n```";
        let AgenticBubbleOutput::Connections { ops } =
            validate_connect_response(raw).expect("valid plan")
        else {
            panic!("expected a connection plan");
        };
        assert_eq!(
            ops[0],
            PatcherConnectOp::Connect {
                from_node: "trig-in".to_string(),
                from_outlet: 0,
                to_node: "created-3".to_string(),
                to_arg: 0,
                why: "trigger".to_string(),
            }
        );
        assert_eq!(
            ops[1],
            PatcherConnectOp::Inline {
                value: "0.8".to_string(),
                to_node: "created-3".to_string(),
                to_arg: 2,
                why: "decay needs a value".to_string(),
            }
        );
    }

    #[test]
    fn connect_plan_rejects_defmacro_output() {
        let error =
            validate_connect_response("(defmacro smooth (sig amt) (mix sig amt 0.5))").unwrap_err();
        assert!(error.contains("not valid JSON"), "{error}");
    }

    #[test]
    fn connect_prompt_shares_no_text_with_the_create_prompt() {
        let request = AgenticBubbleRequest {
            prompt: "connect it".to_string(),
            suggested_macro_name: "voice".to_string(),
            follow_up: None,
            connect: Some(AgenticBubbleConnect {
                subject_node_id: "created-3".to_string(),
                context: "created-3  voice  kind=MacroInstance\n  in  0  trig  free\n".to_string(),
            }),
            history: Vec::new(),
        };
        let connect = request.connect.clone().expect("connect");
        let system = connect_system_prompt();
        let user = connect_user_prompt(&request, &connect, None);
        assert!(system.contains("{\"ops\": [ ... ]}"));
        assert!(system.contains("marks `free`"));
        assert!(!system.contains("defmacro"));
        assert!(user.contains("Wire node created-3"));
        assert!(user.contains("in  0  trig  free"));
    }
}
