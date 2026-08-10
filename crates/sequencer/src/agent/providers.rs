use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::actions::AgentSessionContext;
use super::protocol::{ToolCall, ToolCallOutcome, ToolSpec};

const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const GEMINI_API_KEY_ENV: &str = "GEMINI_API_KEY";
const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_KEY";
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const OPENAI_MODEL_ENV: &str = "SEQUENCER_OPENAI_MODEL";
const GEMINI_MODEL_ENV: &str = "SEQUENCER_GEMINI_MODEL";
const DEEPSEEK_MODEL_ENV: &str = "SEQUENCER_DEEPSEEK_MODEL";
const ANTHROPIC_MODEL_ENV: &str = "SEQUENCER_ANTHROPIC_MODEL";
/// `max_tokens` is required by the Messages API and caps thinking *plus*
/// response text. Thinking is on by default on Claude Opus 5, so a tight cap
/// truncates mid-answer; these turns are non-streaming, so stay under the
/// SDK/HTTP timeout budget rather than reaching for the 128K ceiling.
pub const ANTHROPIC_MAX_TOKENS: u32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentProviderKind {
    OpenAi,
    Gemini,
    DeepSeek,
    Anthropic,
}

impl AgentProviderKind {
    pub fn display_name(self) -> &'static str {
        match self {
            AgentProviderKind::OpenAi => "OpenAI",
            AgentProviderKind::Gemini => "Gemini",
            AgentProviderKind::DeepSeek => "DeepSeek",
            AgentProviderKind::Anthropic => "Anthropic",
        }
    }

    pub fn api_key_env(self) -> &'static str {
        match self {
            AgentProviderKind::OpenAi => OPENAI_API_KEY_ENV,
            AgentProviderKind::Gemini => GEMINI_API_KEY_ENV,
            AgentProviderKind::DeepSeek => DEEPSEEK_API_KEY_ENV,
            AgentProviderKind::Anthropic => ANTHROPIC_API_KEY_ENV,
        }
    }

    pub fn model_override_env(self) -> &'static str {
        match self {
            AgentProviderKind::OpenAi => OPENAI_MODEL_ENV,
            AgentProviderKind::Gemini => GEMINI_MODEL_ENV,
            AgentProviderKind::DeepSeek => DEEPSEEK_MODEL_ENV,
            AgentProviderKind::Anthropic => ANTHROPIC_MODEL_ENV,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelCapability {
    Balanced,
    Fast,
    Cheap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelPreset {
    pub id: String,
    pub display_name: String,
    pub provider: AgentProviderKind,
    pub capability: ModelCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAvailability {
    pub provider: AgentProviderKind,
    pub api_key_present: bool,
    pub selected_model: String,
    pub available_models: Vec<AgentModelPreset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProviderState {
    pub selected_provider: AgentProviderKind,
    pub providers: Vec<ProviderAvailability>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AgentMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: AgentMessageRole,
    pub content: String,
    pub tool_name: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRequest {
    pub model: String,
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolSpec>,
    pub session_context: AgentSessionContext,
}

pub fn default_model_presets() -> Vec<AgentModelPreset> {
    vec![
        AgentModelPreset {
            id: "gpt-5.5".to_string(),
            display_name: "GPT-5.5".to_string(),
            provider: AgentProviderKind::OpenAi,
            capability: ModelCapability::Balanced,
        },
        AgentModelPreset {
            id: "gpt-5.6-luna".to_string(),
            display_name: "GPT-5.6 Luna".to_string(),
            provider: AgentProviderKind::OpenAi,
            capability: ModelCapability::Cheap,
        },
        AgentModelPreset {
            id: "gpt-5-mini".to_string(),
            display_name: "GPT-5 mini".to_string(),
            provider: AgentProviderKind::OpenAi,
            capability: ModelCapability::Fast,
        },
        AgentModelPreset {
            id: "gpt-5-nano".to_string(),
            display_name: "GPT-5 nano".to_string(),
            provider: AgentProviderKind::OpenAi,
            capability: ModelCapability::Cheap,
        },
        AgentModelPreset {
            id: "gemini-3-flash-preview".to_string(),
            display_name: "Gemini 3 Flash Preview".to_string(),
            provider: AgentProviderKind::Gemini,
            capability: ModelCapability::Cheap,
        },
        AgentModelPreset {
            id: "gemini-3.5-flash".to_string(),
            display_name: "Gemini 3.5 Flash".to_string(),
            provider: AgentProviderKind::Gemini,
            capability: ModelCapability::Cheap,
        },
        AgentModelPreset {
            id: "gemini-2.5-pro".to_string(),
            display_name: "Gemini 2.5 Pro".to_string(),
            provider: AgentProviderKind::Gemini,
            capability: ModelCapability::Balanced,
        },
        AgentModelPreset {
            id: "gemini-2.5-flash".to_string(),
            display_name: "Gemini 2.5 Flash".to_string(),
            provider: AgentProviderKind::Gemini,
            capability: ModelCapability::Cheap,
        },
        AgentModelPreset {
            id: "gemini-2.5-flash-lite".to_string(),
            display_name: "Gemini 2.5 Flash Lite".to_string(),
            provider: AgentProviderKind::Gemini,
            capability: ModelCapability::Cheap,
        },
        // Anthropic model ids carry no date suffix — `claude-opus-5`, not
        // `claude-opus-5-20260101`. A suffixed id 404s at the Messages API.
        AgentModelPreset {
            id: "claude-opus-5".to_string(),
            display_name: "Claude Opus 5".to_string(),
            provider: AgentProviderKind::Anthropic,
            capability: ModelCapability::Balanced,
        },
        AgentModelPreset {
            id: "claude-fable-5".to_string(),
            display_name: "Claude Fable 5".to_string(),
            provider: AgentProviderKind::Anthropic,
            capability: ModelCapability::Balanced,
        },
        AgentModelPreset {
            id: "claude-sonnet-5".to_string(),
            display_name: "Claude Sonnet 5".to_string(),
            provider: AgentProviderKind::Anthropic,
            capability: ModelCapability::Balanced,
        },
        AgentModelPreset {
            id: "claude-haiku-4-5".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            provider: AgentProviderKind::Anthropic,
            capability: ModelCapability::Fast,
        },
        AgentModelPreset {
            id: "deepseek-v4-pro".to_string(),
            display_name: "DeepSeek V4 Pro".to_string(),
            provider: AgentProviderKind::DeepSeek,
            capability: ModelCapability::Balanced,
        },
        AgentModelPreset {
            id: "deepseek-v4-flash".to_string(),
            display_name: "DeepSeek V4 Flash".to_string(),
            provider: AgentProviderKind::DeepSeek,
            capability: ModelCapability::Fast,
        },
    ]
}

impl AgentProviderState {
    pub fn from_env() -> Self {
        let models = default_model_presets();
        let providers = [
            AgentProviderKind::OpenAi,
            AgentProviderKind::Gemini,
            AgentProviderKind::DeepSeek,
            AgentProviderKind::Anthropic,
        ]
        .into_iter()
        .map(|provider| ProviderAvailability {
            provider,
            api_key_present: std::env::var(provider.api_key_env())
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false),
            selected_model: provider_selected_model(provider, &models),
            available_models: models
                .iter()
                .filter(|preset| preset.provider == provider)
                .cloned()
                .collect(),
        })
        .collect::<Vec<_>>();

        let selected_provider = providers
            .iter()
            .find(|entry| entry.api_key_present)
            .map(|entry| entry.provider)
            .unwrap_or(AgentProviderKind::OpenAi);

        Self {
            selected_provider,
            providers,
        }
    }

    pub fn selected_model(&self) -> Option<&str> {
        self.providers
            .iter()
            .find(|entry| entry.provider == self.selected_provider)
            .map(|entry| entry.selected_model.as_str())
    }
}

fn provider_selected_model(provider: AgentProviderKind, presets: &[AgentModelPreset]) -> String {
    if let Ok(value) = std::env::var(provider.model_override_env()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    presets
        .iter()
        .find(|preset| {
            preset.provider == provider
                && matches!(
                    preset.capability,
                    ModelCapability::Balanced | ModelCapability::Cheap
                )
        })
        .map(|preset| preset.id.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn build_openai_responses_payload(request: &AgentTurnRequest) -> Value {
    json!({
        "model": request.model,
        "input": request.messages.iter().map(openai_message_json).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(openai_tool_json).collect::<Vec<_>>(),
        "instructions": request.system_prompt,
    })
}

pub fn build_gemini_generate_content_payload(request: &AgentTurnRequest) -> Value {
    json!({
        "systemInstruction": {
            "parts": [{ "text": request.system_prompt }]
        },
        "contents": request.messages.iter().map(gemini_message_json).collect::<Vec<_>>(),
        "tools": [{
            "functionDeclarations": request.tools.iter().map(gemini_tool_json).collect::<Vec<_>>()
        }]
    })
}

/// Anthropic's Messages API takes the system prompt as a top-level field
/// rather than a `system`-role message, and requires an explicit `max_tokens`.
pub fn build_anthropic_messages_payload(request: &AgentTurnRequest) -> Value {
    json!({
        "model": request.model,
        "max_tokens": ANTHROPIC_MAX_TOKENS,
        "system": request.system_prompt,
        "messages": anthropic_messages(&request.messages),
        "tools": request.tools.iter().map(anthropic_tool_json).collect::<Vec<_>>(),
    })
}

/// Messages API history. The transcript's tool rows are dropped for the same
/// reason as on OpenAI: they no longer carry the `tool_use` ids that pair them
/// with an assistant turn. System rows become user turns — the mid-conversation
/// `role: "system"` message is model-gated, and a 400 there would be worse than
/// a slightly weaker signal.
pub fn anthropic_messages(messages: &[AgentMessage]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for message in messages {
        if matches!(message.role, AgentMessageRole::Tool) {
            continue;
        }
        let role = match message.role {
            AgentMessageRole::Assistant => "assistant",
            _ => "user",
        };
        // The first message must be a user turn, so drop any leading assistant
        // rows rather than letting the API reject the whole request.
        if out.is_empty() && role == "assistant" {
            continue;
        }
        out.push(json!({
            "role": role,
            "content": [{ "type": "text", "text": message.content }],
        }));
    }
    out
}

fn anthropic_tool_json(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "input_schema": spec.input_schema,
    })
}

pub fn normalize_openai_tool_call(name: &str, arguments_json: &str) -> Result<ToolCall, String> {
    let arguments = if arguments_json.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments_json)
            .map_err(|error| format!("Invalid OpenAI tool arguments: {error}"))?
    };
    Ok(ToolCall {
        name: name.to_string(),
        arguments,
    })
}

pub fn normalize_gemini_tool_call(name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        name: name.to_string(),
        arguments,
    }
}

pub fn tool_outcome_as_assistant_text(outcome: &ToolCallOutcome) -> String {
    format!(
        "tool={} ok={} summary={}\n{}",
        outcome.name, outcome.ok, outcome.summary, outcome.content
    )
}

fn openai_tool_json(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.input_schema,
    })
}

fn gemini_tool_json(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.input_schema,
    })
}

fn openai_message_json(message: &AgentMessage) -> Value {
    match message.role {
        AgentMessageRole::Assistant => json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": message.content }],
        }),
        AgentMessageRole::Tool => json!({
            "type": "message",
            "role": "tool",
            "content": [{ "type": "output_text", "text": message.content }],
        }),
        _ => json!({
            "type": "message",
            "role": role_name(message.role),
            "content": [{ "type": "input_text", "text": message.content }],
        }),
    }
}

fn gemini_message_json(message: &AgentMessage) -> Value {
    json!({
        "role": gemini_role_name(message.role),
        "parts": [{ "text": message.content }],
    })
}

fn role_name(role: AgentMessageRole) -> &'static str {
    match role {
        AgentMessageRole::System => "system",
        AgentMessageRole::User => "user",
        AgentMessageRole::Assistant => "assistant",
        AgentMessageRole::Tool => "tool",
    }
}

fn gemini_role_name(role: AgentMessageRole) -> &'static str {
    match role {
        AgentMessageRole::System => "user",
        AgentMessageRole::User => "user",
        AgentMessageRole::Assistant => "model",
        AgentMessageRole::Tool => "user",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_anthropic_messages_payload, build_gemini_generate_content_payload,
        build_openai_responses_payload, default_model_presets, normalize_openai_tool_call,
        AgentMessage, AgentMessageRole, AgentProviderKind, AgentProviderState, AgentTurnRequest,
        ModelCapability,
    };
    use crate::agent::actions::AgentSessionContext;
    use crate::agent::protocol::AgentToolRuntime;

    #[test]
    fn provider_state_contains_both_backends() {
        let state = AgentProviderState::from_env();
        assert_eq!(state.providers.len(), 4);
        assert!(state
            .providers
            .iter()
            .any(|entry| entry.provider == AgentProviderKind::OpenAi));
        assert!(state
            .providers
            .iter()
            .any(|entry| entry.provider == AgentProviderKind::Gemini));
        assert!(state
            .providers
            .iter()
            .any(|entry| entry.provider == AgentProviderKind::DeepSeek));
        assert!(state
            .providers
            .iter()
            .any(|entry| entry.provider == AgentProviderKind::Anthropic));
    }

    #[test]
    fn anthropic_presets_are_available() {
        let presets = default_model_presets();
        for id in [
            "claude-opus-5",
            "claude-fable-5",
            "claude-sonnet-5",
            "claude-haiku-4-5",
        ] {
            let preset = presets
                .iter()
                .find(|preset| preset.id == id)
                .unwrap_or_else(|| panic!("{id} preset"));
            assert_eq!(preset.provider, AgentProviderKind::Anthropic);
        }
        assert_eq!(
            AgentProviderKind::Anthropic.api_key_env(),
            "ANTHROPIC_API_KEY"
        );
        // Model ids are complete as written — a date suffix would 404.
        assert!(!presets
            .iter()
            .any(|preset| preset.provider == AgentProviderKind::Anthropic
                && preset.id.contains("-2026")));
    }

    #[test]
    fn anthropic_payload_lifts_the_system_prompt_out_of_messages() {
        let request = AgentTurnRequest {
            model: "claude-opus-5".to_string(),
            system_prompt: "You are helpful.".to_string(),
            messages: vec![
                // A leading assistant row would make the API reject the whole
                // request, so it must be dropped rather than forwarded.
                AgentMessage {
                    role: AgentMessageRole::Assistant,
                    content: "stale".to_string(),
                    tool_name: None,
                    reasoning_content: None,
                },
                AgentMessage {
                    role: AgentMessageRole::User,
                    content: "make a bright pad".to_string(),
                    tool_name: None,
                    reasoning_content: None,
                },
                AgentMessage {
                    role: AgentMessageRole::Tool,
                    content: "tool chatter".to_string(),
                    tool_name: Some("lookup_dgen_docs".to_string()),
                    reasoning_content: None,
                },
            ],
            tools: Vec::new(),
            session_context: AgentSessionContext::default(),
        };
        let payload = build_anthropic_messages_payload(&request);
        assert_eq!(payload["system"], json!("You are helpful."));
        assert_eq!(payload["max_tokens"], json!(super::ANTHROPIC_MAX_TOKENS));
        let messages = payload["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(
            messages[0]["content"][0]["text"],
            json!("make a bright pad")
        );
    }

    #[test]
    fn deepseek_v4_pro_is_available() {
        let presets = default_model_presets();
        let preset = presets
            .iter()
            .find(|preset| preset.id == "deepseek-v4-pro")
            .expect("deepseek preset");
        assert_eq!(preset.provider, AgentProviderKind::DeepSeek);
        assert_eq!(AgentProviderKind::DeepSeek.api_key_env(), "DEEPSEEK_KEY");
    }

    #[test]
    fn deepseek_v4_flash_is_available() {
        let presets = default_model_presets();
        let preset = presets
            .iter()
            .find(|preset| preset.id == "deepseek-v4-flash")
            .expect("deepseek flash preset");
        assert_eq!(preset.provider, AgentProviderKind::DeepSeek);
        assert_eq!(preset.capability, ModelCapability::Fast);
    }

    #[test]
    fn gpt_5_6_luna_is_available() {
        let presets = default_model_presets();
        let preset = presets
            .iter()
            .find(|preset| preset.id == "gpt-5.6-luna")
            .expect("GPT-5.6 Luna preset");
        assert_eq!(preset.provider, AgentProviderKind::OpenAi);
        assert_eq!(preset.capability, ModelCapability::Cheap);
    }

    #[test]
    fn gemini_3_5_flash_replaces_latest_flash_alias() {
        let presets = default_model_presets();
        let preset = presets
            .iter()
            .find(|preset| preset.id == "gemini-3.5-flash")
            .expect("gemini 3.5 flash preset");
        assert_eq!(preset.provider, AgentProviderKind::Gemini);
        assert_eq!(preset.capability, ModelCapability::Cheap);
        assert!(!presets
            .iter()
            .any(|preset| preset.id == "gemini-flash-latest"));
    }

    #[test]
    fn openai_payload_contains_tools() {
        let runtime = AgentToolRuntime::load_default().expect("runtime");
        let request = AgentTurnRequest {
            model: "gpt-5.5".to_string(),
            system_prompt: "You are helpful.".to_string(),
            messages: vec![AgentMessage {
                role: AgentMessageRole::User,
                content: "make a bright pad".to_string(),
                tool_name: None,
                reasoning_content: None,
            }],
            tools: runtime.specs(),
            session_context: AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_ui_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
                current_instrument_preset_schema: None,
            },
        };
        let payload = build_openai_responses_payload(&request);
        assert_eq!(payload["model"], json!("gpt-5.5"));
        assert!(payload["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()));
    }

    #[test]
    fn openai_payload_uses_output_text_for_assistant_history() {
        let request = AgentTurnRequest {
            model: "gpt-5.5".to_string(),
            system_prompt: "You are helpful.".to_string(),
            messages: vec![
                AgentMessage {
                    role: AgentMessageRole::User,
                    content: "make an organ".to_string(),
                    tool_name: None,
                    reasoning_content: None,
                },
                AgentMessage {
                    role: AgentMessageRole::Assistant,
                    content: "```dgenlisp\n(out 0 1 @name audio)\n```".to_string(),
                    tool_name: None,
                    reasoning_content: None,
                },
                AgentMessage {
                    role: AgentMessageRole::System,
                    content: "compile error: parse error".to_string(),
                    tool_name: None,
                    reasoning_content: None,
                },
            ],
            tools: Vec::new(),
            session_context: AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_ui_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
                current_instrument_preset_schema: None,
            },
        };
        let payload = build_openai_responses_payload(&request);
        assert_eq!(
            payload["input"][0]["content"][0]["type"],
            json!("input_text")
        );
        assert_eq!(
            payload["input"][1]["content"][0]["type"],
            json!("output_text")
        );
        assert_eq!(
            payload["input"][2]["content"][0]["type"],
            json!("input_text")
        );
    }

    #[test]
    fn gemini_payload_contains_function_declarations() {
        let runtime = AgentToolRuntime::load_default().expect("runtime");
        let request = AgentTurnRequest {
            model: "gemini-2.5-flash".to_string(),
            system_prompt: "You are helpful.".to_string(),
            messages: vec![AgentMessage {
                role: AgentMessageRole::User,
                content: "make a bright pad".to_string(),
                tool_name: None,
                reasoning_content: None,
            }],
            tools: runtime.specs(),
            session_context: AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_ui_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
                current_instrument_preset_schema: None,
            },
        };
        let payload = build_gemini_generate_content_payload(&request);
        assert!(payload["tools"][0]["functionDeclarations"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()));
    }

    #[test]
    fn normalize_openai_arguments_parses_json_string() {
        let call =
            normalize_openai_tool_call("lookup_dgen_docs", r#"{"query":"biquad","limit":2}"#)
                .expect("normalize");
        assert_eq!(call.name, "lookup_dgen_docs");
        assert_eq!(call.arguments["query"], json!("biquad"));
    }
}
