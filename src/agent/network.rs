use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};

use super::actions::{AgentAppAction, AgentSessionContext};
use super::protocol::{AgentToolRuntime, ToolCall, ToolCallOutcome, ToolSpec};
use super::providers::{AgentMessage, AgentMessageRole, AgentProviderKind, AgentTurnRequest};

const OPENAI_CHAT_COMPLETIONS_URL: &str = "https://api.openai.com/v1/chat/completions";
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_REPEAT_TOOL_FAILURES: usize = 2;
const MAX_REPEAT_TOOL_CALL_ROUNDS: usize = 2;

#[derive(Debug, Clone)]
pub struct AgentTurnResult {
    pub text: String,
    pub tool_outcomes: Vec<ToolCallOutcome>,
    pub pending_actions: Vec<AgentAppAction>,
}

#[derive(Debug, Clone)]
pub struct AgentTurnError {
    pub message: String,
    pub tool_outcomes: Vec<ToolCallOutcome>,
}

pub struct AgentNetworkClient {
    http: Client,
    tools: AgentToolRuntime,
}

impl AgentNetworkClient {
    pub fn load_default() -> Result<Self, String> {
        let http = Client::builder()
            .build()
            .map_err(|error| format!("Failed to build HTTP client: {error}"))?;
        Ok(Self {
            http,
            tools: AgentToolRuntime::load_default()?,
        })
    }

    pub fn execute_turn(
        &self,
        provider: AgentProviderKind,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
        session_context: AgentSessionContext,
    ) -> Result<AgentTurnResult, AgentTurnError> {
        let request = AgentTurnRequest {
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            messages: messages.to_vec(),
            tools: self.tools.specs(),
            session_context,
        };

        match provider {
            AgentProviderKind::OpenAi => self.execute_openai_turn(&request),
            AgentProviderKind::Gemini => self.execute_gemini_turn(&request),
        }
    }

    fn execute_openai_turn(
        &self,
        request: &AgentTurnRequest,
    ) -> Result<AgentTurnResult, AgentTurnError> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| AgentTurnError {
            message: "Missing required OPENAI_API_KEY.".to_string(),
            tool_outcomes: Vec::new(),
        })?;
        let mut messages = openai_messages(request);
        let tools = openai_tools(&request.tools);
        let mut tool_outcomes = Vec::new();
        let mut pending_actions = Vec::new();
        let mut last_failure_signature = None::<String>;
        let mut repeated_failure_rounds = 0usize;
        let mut last_tool_signature = None::<String>;
        let mut repeated_tool_call_rounds = 0usize;

        for _ in 0..MAX_TOOL_ROUNDS {
            let payload = json!({
                "model": request.model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto"
            });
            let response = self
                .http
                .post(OPENAI_CHAT_COMPLETIONS_URL)
                .header(AUTHORIZATION, format!("Bearer {api_key}"))
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .map_err(|error| AgentTurnError {
                    message: format!("OpenAI request failed: {error}"),
                    tool_outcomes: tool_outcomes.clone(),
                })?;
            let status = response.status();
            if !status.is_success() {
                let body = response
                    .text()
                    .unwrap_or_else(|_| "<failed to read response body>".to_string());
                return Err(AgentTurnError {
                    message: format!("OpenAI request failed: HTTP {status} body: {body}"),
                    tool_outcomes: tool_outcomes.clone(),
                });
            }
            let response: OpenAiChatCompletionResponse =
                response.json().map_err(|error| AgentTurnError {
                    message: format!("Failed to decode OpenAI response: {error}"),
                    tool_outcomes: tool_outcomes.clone(),
                })?;

            let message = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| AgentTurnError {
                    message: "OpenAI returned no choices.".to_string(),
                    tool_outcomes: tool_outcomes.clone(),
                })?
                .message;

            let tool_calls = message.tool_calls.unwrap_or_default();
            let assistant_content = message.content.unwrap_or_default();
            if !tool_calls.is_empty() {
                let tool_signature = openai_tool_call_signature(&tool_calls);
                if last_tool_signature.as_deref() == Some(tool_signature.as_str()) {
                    repeated_tool_call_rounds += 1;
                } else {
                    repeated_tool_call_rounds = 1;
                    last_tool_signature = Some(tool_signature.clone());
                }
                if repeated_tool_call_rounds >= MAX_REPEAT_TOOL_CALL_ROUNDS {
                    return Err(format!(
                        "Agent repeated the same tool-call plan: {tool_signature}"
                    )
                    .into_error(tool_outcomes));
                }

                messages.push(json!({
                    "role": "assistant",
                    "content": assistant_content,
                    "tool_calls": tool_calls.iter().map(openai_tool_call_json).collect::<Vec<_>>(),
                }));

                let mut round_outcomes = Vec::new();
                for tool_call in tool_calls {
                    let call = ToolCall {
                        name: tool_call.function.name.clone(),
                        arguments: if tool_call.function.arguments.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&tool_call.function.arguments).map_err(
                                |error| AgentTurnError {
                                    message: format!(
                                        "OpenAI tool arguments for '{}' were invalid JSON: {error}",
                                        tool_call.function.name
                                    ),
                                    tool_outcomes: tool_outcomes.clone(),
                                },
                            )?
                        },
                    };
                    let outcome = self.tools.execute(call, &request.session_context);
                    pending_actions.extend(outcome.pending_actions.clone());
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_call.id,
                        "content": outcome.content.clone(),
                    }));
                    round_outcomes.push(outcome.clone());
                    tool_outcomes.push(outcome);
                }

                if round_outcomes
                    .iter()
                    .any(|outcome| !outcome.pending_actions.is_empty())
                {
                    return Ok(AgentTurnResult {
                        text: assistant_content,
                        tool_outcomes,
                        pending_actions,
                    });
                }

                if let Some(signature) = repeated_failure_signature(&round_outcomes) {
                    if last_failure_signature.as_deref() == Some(signature.as_str()) {
                        repeated_failure_rounds += 1;
                    } else {
                        repeated_failure_rounds = 1;
                        last_failure_signature = Some(signature.clone());
                    }
                    if repeated_failure_rounds >= MAX_REPEAT_TOOL_FAILURES {
                        return Err(format!(
                            "Agent repeated the same failing tool call: {signature}"
                        )
                        .into_error(tool_outcomes));
                    }
                } else {
                    repeated_failure_rounds = 0;
                    last_failure_signature = None;
                }
                continue;
            }

            return Ok(AgentTurnResult {
                text: assistant_content,
                tool_outcomes,
                pending_actions,
            });
        }

        Err(
            format_tool_loop_error("OpenAI", MAX_TOOL_ROUNDS, last_tool_signature.as_deref())
                .into_error(tool_outcomes),
        )
    }

    fn execute_gemini_turn(
        &self,
        request: &AgentTurnRequest,
    ) -> Result<AgentTurnResult, AgentTurnError> {
        let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| AgentTurnError {
            message: "Missing required GEMINI_API_KEY.".to_string(),
            tool_outcomes: Vec::new(),
        })?;
        let endpoint = format!("{GEMINI_API_BASE}/{}:generateContent", request.model);
        let mut contents = gemini_contents(request);
        let tools = gemini_tools(&request.tools);
        let mut tool_outcomes = Vec::new();
        let mut pending_actions = Vec::new();
        let mut last_failure_signature = None::<String>;
        let mut repeated_failure_rounds = 0usize;
        let mut last_tool_signature = None::<String>;
        let mut repeated_tool_call_rounds = 0usize;

        for _ in 0..MAX_TOOL_ROUNDS {
            let payload = json!({
                "systemInstruction": {
                    "parts": [{ "text": request.system_prompt }]
                },
                "contents": contents,
                "tools": [{
                    "functionDeclarations": tools
                }]
            });

            let response = self
                .http
                .post(&endpoint)
                .header("x-goog-api-key", api_key.clone())
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .map_err(|error| AgentTurnError {
                    message: format!("Gemini request failed: {error}"),
                    tool_outcomes: tool_outcomes.clone(),
                })?;
            let status = response.status();
            if !status.is_success() {
                let body = response
                    .text()
                    .unwrap_or_else(|_| "<failed to read response body>".to_string());
                return Err(AgentTurnError {
                    message: format!("Gemini request failed: HTTP {status} body: {body}"),
                    tool_outcomes: tool_outcomes.clone(),
                });
            }
            let response: GeminiGenerateContentResponse =
                response.json().map_err(|error| AgentTurnError {
                    message: format!("Failed to decode Gemini response: {error}"),
                    tool_outcomes: tool_outcomes.clone(),
                })?;

            let candidate =
                response
                    .candidates
                    .into_iter()
                    .next()
                    .ok_or_else(|| AgentTurnError {
                        message: "Gemini returned no candidates.".to_string(),
                        tool_outcomes: tool_outcomes.clone(),
                    })?;
            let content = candidate.content.ok_or_else(|| AgentTurnError {
                message: "Gemini returned no content.".to_string(),
                tool_outcomes: tool_outcomes.clone(),
            })?;
            let function_calls = extract_gemini_function_calls(&content.parts);
            let assistant_text = extract_gemini_text(&content.parts);

            contents.push(json!({
                "role": content.role.unwrap_or_else(|| "model".to_string()),
                "parts": content.parts,
            }));

            if function_calls.is_empty() {
                return Ok(AgentTurnResult {
                    text: assistant_text,
                    tool_outcomes,
                    pending_actions,
                });
            }

            let tool_signature = gemini_tool_call_signature(&function_calls);
            if last_tool_signature.as_deref() == Some(tool_signature.as_str()) {
                repeated_tool_call_rounds += 1;
            } else {
                repeated_tool_call_rounds = 1;
                last_tool_signature = Some(tool_signature.clone());
            }
            if repeated_tool_call_rounds >= MAX_REPEAT_TOOL_CALL_ROUNDS {
                return Err(
                    format!("Agent repeated the same tool-call plan: {tool_signature}")
                        .into_error(tool_outcomes),
                );
            }

            let mut response_parts = Vec::new();
            let mut round_outcomes = Vec::new();
            for function_call in function_calls {
                let outcome = self.tools.execute(
                    ToolCall {
                        name: function_call.name.clone(),
                        arguments: function_call.args.clone().unwrap_or_else(|| json!({})),
                    },
                    &request.session_context,
                );
                pending_actions.extend(outcome.pending_actions.clone());
                response_parts.push(json!({
                    "functionResponse": {
                        "name": function_call.name,
                        "response": {
                            "summary": outcome.summary,
                            "content": outcome.content,
                            "ok": outcome.ok
                        }
                    }
                }));
                round_outcomes.push(outcome.clone());
                tool_outcomes.push(outcome);
            }

            if round_outcomes
                .iter()
                .any(|outcome| !outcome.pending_actions.is_empty())
            {
                return Ok(AgentTurnResult {
                    text: assistant_text,
                    tool_outcomes,
                    pending_actions,
                });
            }

            if let Some(signature) = repeated_failure_signature(&round_outcomes) {
                if last_failure_signature.as_deref() == Some(signature.as_str()) {
                    repeated_failure_rounds += 1;
                } else {
                    repeated_failure_rounds = 1;
                    last_failure_signature = Some(signature.clone());
                }
                if repeated_failure_rounds >= MAX_REPEAT_TOOL_FAILURES {
                    return Err(
                        format!("Agent repeated the same failing tool call: {signature}")
                            .into_error(tool_outcomes),
                    );
                }
            } else {
                repeated_failure_rounds = 0;
                last_failure_signature = None;
            }

            contents.push(json!({
                "role": "user",
                "parts": response_parts,
            }));
        }

        Err(
            format_tool_loop_error("Gemini", MAX_TOOL_ROUNDS, last_tool_signature.as_deref())
                .into_error(tool_outcomes),
        )
    }
}

trait IntoAgentTurnError {
    fn into_error(self, tool_outcomes: Vec<ToolCallOutcome>) -> AgentTurnError;
}

impl IntoAgentTurnError for String {
    fn into_error(self, tool_outcomes: Vec<ToolCallOutcome>) -> AgentTurnError {
        AgentTurnError {
            message: self,
            tool_outcomes,
        }
    }
}

fn openai_messages(request: &AgentTurnRequest) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system_prompt,
    })];
    for message in &request.messages {
        if matches!(message.role, AgentMessageRole::Tool) {
            // Tool outputs recorded in the UI transcript do not retain the original
            // tool_call_id linkage required by OpenAI across later turns.
            continue;
        }
        let role = match message.role {
            AgentMessageRole::System => "system",
            AgentMessageRole::User => "user",
            AgentMessageRole::Assistant => "assistant",
            AgentMessageRole::Tool => unreachable!("tool messages are filtered above"),
        };
        let object = json!({
            "role": role,
            "content": message.content,
        });
        messages.push(object);
    }
    messages
}

fn openai_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "type": "function",
                "function": {
                    "name": spec.name,
                    "description": spec.description,
                    "parameters": sanitize_openai_schema(&spec.input_schema)
                }
            })
        })
        .collect()
}

fn openai_tool_call_json(tool_call: &OpenAiToolCall) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.function.name,
            "arguments": tool_call.function.arguments,
        }
    })
}

fn sanitize_openai_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if key == "properties" {
                    if let Value::Object(properties) = child {
                        let mut sanitized_properties = serde_json::Map::new();
                        for (prop_name, prop_schema) in properties {
                            sanitized_properties
                                .insert(prop_name.clone(), sanitize_openai_schema(prop_schema));
                        }
                        out.insert(key.clone(), Value::Object(sanitized_properties));
                    }
                    continue;
                }

                if matches!(
                    key.as_str(),
                    "type" | "description" | "required" | "enum" | "items" | "additionalProperties"
                ) {
                    out.insert(key.clone(), sanitize_openai_schema(child));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_openai_schema).collect()),
        _ => value.clone(),
    }
}

fn gemini_contents(request: &AgentTurnRequest) -> Vec<Value> {
    request
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                AgentMessageRole::Assistant => "model",
                _ => "user",
            };
            json!({
                "role": role,
                "parts": [{ "text": message.content }]
            })
        })
        .collect()
}

fn gemini_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": sanitize_gemini_schema(&spec.input_schema),
            })
        })
        .collect()
}

fn sanitize_gemini_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if key == "properties" {
                    if let Value::Object(properties) = child {
                        let mut sanitized_properties = serde_json::Map::new();
                        for (prop_name, prop_schema) in properties {
                            sanitized_properties
                                .insert(prop_name.clone(), sanitize_gemini_schema(prop_schema));
                        }
                        out.insert(key.clone(), Value::Object(sanitized_properties));
                    }
                    continue;
                }

                if matches!(
                    key.as_str(),
                    "type" | "description" | "required" | "enum" | "items"
                ) {
                    out.insert(key.clone(), sanitize_gemini_schema(child));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_gemini_schema).collect()),
        _ => value.clone(),
    }
}

fn extract_gemini_text(parts: &[GeminiPart]) -> String {
    parts
        .iter()
        .filter_map(|part| part.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_gemini_function_calls(parts: &[GeminiPart]) -> Vec<GeminiFunctionCall> {
    parts
        .iter()
        .filter_map(|part| part.function_call.clone())
        .collect()
}

fn repeated_failure_signature(outcomes: &[ToolCallOutcome]) -> Option<String> {
    if outcomes.is_empty() || outcomes.iter().any(|outcome| outcome.ok) {
        return None;
    }
    Some(
        outcomes
            .iter()
            .map(|outcome| format!("{}: {}", outcome.name, outcome.summary))
            .collect::<Vec<_>>()
            .join(" | "),
    )
}

fn openai_tool_call_signature(tool_calls: &[OpenAiToolCall]) -> String {
    tool_calls
        .iter()
        .map(|call| format!("{}({})", call.function.name, call.function.arguments))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn gemini_tool_call_signature(tool_calls: &[GeminiFunctionCall]) -> String {
    tool_calls
        .iter()
        .map(|call| {
            format!(
                "{}({})",
                call.name,
                call.args.clone().unwrap_or_else(|| json!({}))
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn format_tool_loop_error(
    provider: &str,
    max_rounds: usize,
    last_tool_signature: Option<&str>,
) -> String {
    match last_tool_signature {
        Some(signature) => format!(
            "{provider} tool loop exceeded maximum rounds ({max_rounds}). Last tool-call plan: {signature}"
        ),
        None => format!("{provider} tool loop exceeded maximum rounds ({max_rounds})."),
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type", default = "openai_tool_call_type")]
    call_type: String,
    function: OpenAiToolFunction,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct OpenAiToolFunction {
    name: String,
    arguments: String,
}

fn openai_tool_call_type() -> String {
    "function".to_string()
}

#[derive(Debug, Deserialize)]
struct GeminiGenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    role: Option<String>,
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct GeminiPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        extract_gemini_function_calls, extract_gemini_text, gemini_tool_call_signature,
        openai_messages, openai_tool_call_json, openai_tool_call_type, openai_tools,
        repeated_failure_signature, sanitize_gemini_schema, sanitize_openai_schema,
        GeminiFunctionCall, GeminiPart, OpenAiToolCall, OpenAiToolFunction,
    };
    use crate::agent::actions::AgentSessionContext;
    use crate::agent::protocol::AgentToolRuntime;
    use crate::agent::protocol::{ToolCallOutcome, ToolSpec};
    use crate::agent::providers::{AgentMessage, AgentMessageRole, AgentTurnRequest};

    #[test]
    fn extract_gemini_parts() {
        let parts = vec![
            GeminiPart {
                text: Some("hi".to_string()),
                function_call: None,
                thought_signature: None,
            },
            GeminiPart {
                text: None,
                function_call: Some(GeminiFunctionCall {
                    name: "lookup_dgen_docs".to_string(),
                    args: Some(json!({"query": "biquad"})),
                }),
                thought_signature: Some("sig123".to_string()),
            },
        ];
        assert_eq!(extract_gemini_text(&parts), "hi");
        assert_eq!(extract_gemini_function_calls(&parts).len(), 1);
    }

    #[test]
    fn sanitize_gemini_schema_strips_extra_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "count",
                    "minimum": 1,
                    "default": 5
                }
            },
            "required": ["limit"],
            "default": {}
        });
        let sanitized = sanitize_gemini_schema(&schema);
        assert_eq!(sanitized["type"], json!("object"));
        assert!(sanitized["default"].is_null());
        assert!(sanitized["properties"]["limit"]["minimum"].is_null());
        assert!(sanitized["properties"]["limit"]["default"].is_null());
    }

    #[test]
    fn sanitize_openai_schema_strips_unsupported_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "search term",
                    "default": "osc"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1
                }
            },
            "anyOf": [
                { "required": ["query"] }
            ]
        });
        let sanitized = sanitize_openai_schema(&schema);
        assert_eq!(sanitized["type"], json!("object"));
        assert!(sanitized["anyOf"].is_null());
        assert!(sanitized["properties"]["query"]["default"].is_null());
        assert!(sanitized["properties"]["limit"]["minimum"].is_null());
    }

    #[test]
    fn openai_tools_omit_strict_flag() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let tools = openai_tools(&runtime.specs());
        assert!(tools
            .iter()
            .all(|tool| tool["function"]["strict"].is_null()));
    }

    #[test]
    fn openai_tool_call_serializes_with_type() {
        let tool_call = OpenAiToolCall {
            id: "call_123".to_string(),
            call_type: openai_tool_call_type(),
            function: OpenAiToolFunction {
                name: "lookup_dgen_docs".to_string(),
                arguments: r#"{"query":"filter"}"#.to_string(),
            },
        };
        let json_value = serde_json::to_value(&tool_call).expect("serialize tool call");
        assert_eq!(json_value["type"], json!("function"));
    }

    #[test]
    fn openai_tool_call_json_includes_type() {
        let tool_call = OpenAiToolCall {
            id: "call_123".to_string(),
            call_type: openai_tool_call_type(),
            function: OpenAiToolFunction {
                name: "lookup_dgen_docs".to_string(),
                arguments: r#"{"query":"filter"}"#.to_string(),
            },
        };
        let json_value = openai_tool_call_json(&tool_call);
        assert_eq!(json_value["type"], json!("function"));
        assert_eq!(json_value["function"]["name"], json!("lookup_dgen_docs"));
    }

    #[test]
    fn openai_messages_skip_persisted_tool_transcript_entries() {
        let request = AgentTurnRequest {
            model: "gpt-5.4".to_string(),
            system_prompt: "You are helpful.".to_string(),
            messages: vec![
                AgentMessage {
                    role: AgentMessageRole::User,
                    content: "make a patch".to_string(),
                    tool_name: None,
                },
                AgentMessage {
                    role: AgentMessageRole::Tool,
                    content: "lookup_dgen_docs [ok]\noperator saw".to_string(),
                    tool_name: Some("lookup_dgen_docs".to_string()),
                },
                AgentMessage {
                    role: AgentMessageRole::System,
                    content: "Applying your last generated change failed.".to_string(),
                    tool_name: None,
                },
            ],
            tools: Vec::<ToolSpec>::new(),
            session_context: AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
                current_instrument_preset_schema: None,
            },
        };
        let messages = openai_messages(&request);
        assert_eq!(messages.len(), 3);
        assert!(messages.iter().all(|msg| msg["role"] != json!("tool")));
    }

    #[test]
    fn gemini_thought_signature_round_trips() {
        let json_value = json!({
            "text": null,
            "functionCall": {
                "name": "list_examples",
                "args": { "kind": "instrument" }
            },
            "thoughtSignature": "abc123"
        });
        let part: GeminiPart = serde_json::from_value(json_value).expect("deserialize part");
        let round_trip = serde_json::to_value(&part).expect("serialize part");
        assert_eq!(round_trip["thoughtSignature"], json!("abc123"));
    }

    #[test]
    fn repeated_failure_signature_only_triggers_for_all_failed_rounds() {
        let outcomes = vec![
            ToolCallOutcome {
                name: "update_current_instrument".to_string(),
                ok: false,
                summary: "compile failed".to_string(),
                content: "compile failed".to_string(),
                pending_actions: Vec::new(),
            },
            ToolCallOutcome {
                name: "read_current_instrument_source".to_string(),
                ok: false,
                summary: "no custom track".to_string(),
                content: "no custom track".to_string(),
                pending_actions: Vec::new(),
            },
        ];
        assert!(repeated_failure_signature(&outcomes).is_some());
    }

    #[test]
    fn gemini_tool_call_signature_is_stable() {
        let tool_calls = vec![
            GeminiFunctionCall {
                name: "lookup_dgen_docs".to_string(),
                args: Some(json!({"query": "horn"})),
            },
            GeminiFunctionCall {
                name: "list_examples".to_string(),
                args: Some(json!({"kind": "instrument"})),
            },
        ];
        let signature = gemini_tool_call_signature(&tool_calls);
        assert!(signature.contains("lookup_dgen_docs"));
        assert!(signature.contains("list_examples"));
    }
}
