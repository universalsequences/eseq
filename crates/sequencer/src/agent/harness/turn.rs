//! One user turn: stream the model's reply, run the tools it calls, send the
//! results back, until it ends its turn. The history is append-only (the
//! model's own blocks go back unchanged, which thinking requires) and stays
//! valid however a turn ends: every `tool_use` gets its `tool_result`, a
//! message cut off mid-way keeps only its text.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::lock;
use super::mcp_client::{ToolBackend, ToolResult};
use super::message::{MessageAccumulator, StopReason, StreamUpdate};
use super::sse::SseParser;
use super::transcript::{Entry, ToolStatus, Transcript};
use crate::agent::network::{format_reqwest_error, ANTHROPIC_MESSAGES_URL, ANTHROPIC_VERSION};
use crate::agent::providers::AgentProviderKind;

/// `fallbacks: "default"`: a request Claude Opus 5.5's safeguards decline is
/// re-run on the fallback model the API picks for the refusal category.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Streams one Messages API request: the real API, or a script in tests.
pub trait ModelTransport: Send + Sync {
    /// POST the JSON `body`, feeding each streamed event to `on_event`;
    /// stops early when it returns false.
    fn stream(&self, body: Vec<u8>, on_event: &mut dyn FnMut(&Value) -> bool) -> Result<(), String>;
}

pub struct AnthropicTransport {
    http: reqwest::blocking::Client,
    api_key: String,
}

impl AnthropicTransport {
    pub fn from_env() -> Result<Self, String> {
        let key_env = AgentProviderKind::Anthropic.api_key_env();
        let api_key = std::env::var(key_env)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| format!("Set {key_env} to talk to Claude."))?;
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            // A long agentic reply streams for minutes; the cap is for a
            // connection that hangs, not for a slow answer.
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .map_err(|error| format_reqwest_error(&error))?;
        Ok(Self { http, api_key })
    }
}

impl ModelTransport for AnthropicTransport {
    fn stream(&self, body: Vec<u8>, on_event: &mut dyn FnMut(&Value) -> bool) -> Result<(), String> {
        let response = self
            .http
            .post(ANTHROPIC_MESSAGES_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .map_err(|error| format!("Could not reach the Claude API: {}", format_reqwest_error(&error)))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|body| body["error"]["message"].as_str().map(str::to_string))
                .unwrap_or(text);
            return Err(format!("Claude API {status}: {message}"));
        }
        let mut parser = SseParser::default();
        for line in BufReader::new(response).lines() {
            let line = line.map_err(|error| format!("The response stream broke: {error}"))?;
            let Some(data) = parser.line(&line) else { continue };
            let Ok(event) = serde_json::from_str::<Value>(&data) else { continue };
            if !on_event(&event) {
                break;
            }
        }
        Ok(())
    }
}

/// What every request of a session shares.
pub struct TurnConfig {
    pub model: String,
    pub effort: String,
    pub max_tokens: u32,
    pub max_rounds: usize,
    pub system: String,
    /// Messages API tool definitions.
    pub tools: Vec<Value>,
    /// The conversation's folder: `conversation.json` and the tools' images.
    pub dir: PathBuf,
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: [Value; 1],
    tools: &'a [Value],
    messages: &'a [Value],
    thinking: Value,
    output_config: Value,
    cache_control: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallbacks: Option<&'static str>,
}

/// The Messages API request for the conversation so far, serialized from
/// borrowed parts (the history holds every image the tools returned). The
/// tools and the system prompt never change within a session, and the
/// automatic cache breakpoint follows the growing history.
pub fn request_body(config: &TurnConfig, messages: &[Value]) -> Vec<u8> {
    serde_json::to_vec(&RequestBody {
        model: &config.model,
        max_tokens: config.max_tokens,
        stream: true,
        system: [json!({ "type": "text", "text": config.system })],
        tools: &config.tools,
        messages,
        thinking: json!({ "type": "adaptive", "display": "summarized" }),
        output_config: json!({ "effort": config.effort }),
        cache_control: json!({ "type": "ephemeral" }),
        // The fallback models are for Opus's refusals; Haiku rejects the field.
        fallbacks: config.model.starts_with("claude-opus").then_some("default"),
    })
    .expect("a JSON request serializes")
}

/// Add the user's words: to the last message when it is already the user's
/// (tool results, or a turn that was stopped before the model said anything).
pub fn push_user_text(history: &mut Vec<Value>, text: &str) {
    let block = json!({ "type": "text", "text": text });
    match history.last_mut() {
        Some(last) if last["role"] == "user" => {
            if let Some(content) = last["content"].as_array_mut() {
                content.push(block);
                return;
            }
        }
        _ => {}
    }
    history.push(json!({ "role": "user", "content": [block] }));
}

/// Run the model until it ends its turn (or `config.max_rounds` tool rounds,
/// or `cancel`). Errors are the API's; tool failures go back to the model.
pub fn run_turn(
    transport: &dyn ModelTransport,
    tools: &mut dyn ToolBackend,
    config: &TurnConfig,
    history: &mut Vec<Value>,
    transcript: &Mutex<Transcript>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let notice = |text: &str| lock(transcript).push(Entry::Notice(text.to_string()));
    // A turn that ends early leaves the tool rows it started unrun.
    let end_early = |history: &mut Vec<Value>, message: &MessageAccumulator| {
        keep_partial(history, message);
        lock(transcript).fail_running_tools("Did not run.");
    };
    for _ in 0..config.max_rounds {
        record(config, history);
        let mut message = MessageAccumulator::default();
        let mut failure = None;
        let streamed = transport.stream(request_body(config, history), &mut |event| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            match message.apply(event) {
                Ok(update) => {
                    let mut transcript = lock(transcript);
                    match update {
                        StreamUpdate::Text(text) => transcript.append_assistant(&text),
                        StreamUpdate::Thinking(text) => transcript.append_thinking(&text),
                        StreamUpdate::ToolStarted { id, name } => transcript.push(Entry::Tool {
                            id,
                            name,
                            input: String::new(),
                            status: ToolStatus::Running,
                            output: String::new(),
                            images: Vec::new(),
                        }),
                        StreamUpdate::Nothing => {}
                    }
                    true
                }
                Err(error) => {
                    failure = Some(error);
                    false
                }
            }
        });
        lock(transcript).add_usage(message.input_tokens, message.output_tokens, message.cache_read_tokens);
        if let Some(error) = failure.or(streamed.err()) {
            end_early(history, &message);
            return Err(error);
        }
        if cancel.load(Ordering::Relaxed) {
            end_early(history, &message);
            notice("Stopped.");
            return Ok(());
        }
        let Some(stop_reason) = message.stop_reason.clone() else {
            end_early(history, &message);
            return Err("The response ended before the message finished.".to_string());
        };
        if stop_reason == StopReason::Refusal {
            // A declined reply may stop anywhere: keep only its text.
            end_early(history, &message);
            notice("Claude declined this request.");
            return Ok(());
        }
        let tool_uses = message.tool_uses();
        history.push(json!({ "role": "assistant", "content": message.content() }));
        record(config, history);
        let unrunnable = match stop_reason {
            StopReason::ToolUse => None,
            StopReason::MaxTokens => Some("The reply reached max_tokens before this call was complete; it did not run."),
            _ => Some("This call did not run."),
        };
        if !tool_uses.is_empty() {
            let mut results = Vec::new();
            for tool in &tool_uses {
                let result = if let Some(reason) = unrunnable {
                    ToolResult::error(reason)
                } else if cancel.load(Ordering::Relaxed) {
                    ToolResult::error("The user stopped the turn before this call ran.")
                } else if let Some(raw) = &tool.invalid_input {
                    ToolResult::error(json!({ "INVALID_JSON": raw }).to_string())
                } else {
                    tools.call(&tool.name, &tool.input)
                };
                let images = save_images(&config.dir.join("images"), &tool.id, &result.images);
                let status = if result.is_error { ToolStatus::Failed } else { ToolStatus::Done };
                let input = tool.invalid_input.clone().unwrap_or_else(|| tool.input.to_string());
                lock(transcript).finish_tool(&tool.id, input, status, result.text(), images);
                results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": tool.id,
                    "content": result.content,
                    "is_error": result.is_error,
                }));
            }
            history.push(json!({ "role": "user", "content": results }));
        }
        match stop_reason {
            StopReason::ToolUse if cancel.load(Ordering::Relaxed) => {
                notice("Stopped.");
                return Ok(());
            }
            StopReason::ToolUse | StopReason::PauseTurn => continue,
            StopReason::MaxTokens => {
                notice("The reply reached its length limit.");
                return Ok(());
            }
            _ => return Ok(()),
        }
    }
    notice(&format!("Stopped after {} tool rounds; say \"continue\" to go on.", config.max_rounds));
    Ok(())
}

/// A message cut off mid-stream keeps only its text: a partial thinking
/// block has no signature and a partial tool call no result.
fn keep_partial(history: &mut Vec<Value>, message: &MessageAccumulator) {
    let text = message.text_blocks();
    if !text.is_empty() {
        history.push(json!({ "role": "assistant", "content": text }));
    }
}

/// Write the conversation so far to `conversation.json` in its folder, for
/// debugging: the messages as sent, thinking included, without image data
/// (the images are in `images/`). Rewritten each round, so a running turn
/// can be read too. A config with no folder records nothing.
pub fn record(config: &TurnConfig, history: &[Value]) {
    if config.dir.as_os_str().is_empty() {
        return;
    }
    let record = json!({
        "model": config.model,
        "effort": config.effort,
        "system": config.system,
        "messages": history.iter().map(without_image_data).collect::<Vec<_>>(),
    });
    if std::fs::create_dir_all(&config.dir).is_ok() {
        let json = serde_json::to_vec_pretty(&record).expect("a JSON record serializes");
        let _ = std::fs::write(config.dir.join("conversation.json"), json);
    }
}

fn without_image_data(value: &Value) -> Value {
    match value {
        Value::Object(map) if map.get("type").and_then(Value::as_str) == Some("image") => {
            json!({ "type": "image", "data": "omitted" })
        }
        Value::Object(map) => Value::Object(map.iter().map(|(key, value)| (key.clone(), without_image_data(value))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(without_image_data).collect()),
        other => other.clone(),
    }
}

fn save_images(dir: &Path, tool_id: &str, images: &[Vec<u8>]) -> Vec<PathBuf> {
    if images.is_empty() || std::fs::create_dir_all(dir).is_err() {
        return Vec::new();
    }
    images
        .iter()
        .enumerate()
        .filter_map(|(index, png)| {
            let path = dir.join(format!("{tool_id}-{index}.png"));
            std::fs::write(&path, png).ok().map(|()| path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that answers each request with the next scripted event list.
    struct Script(Mutex<Vec<Vec<Value>>>);

    impl ModelTransport for Script {
        fn stream(&self, _body: Vec<u8>, on_event: &mut dyn FnMut(&Value) -> bool) -> Result<(), String> {
            let events = self.0.lock().unwrap().remove(0);
            for event in &events {
                if !on_event(event) {
                    break;
                }
            }
            Ok(())
        }
    }

    struct Tools(Vec<(String, Value)>);

    impl ToolBackend for Tools {
        fn tools(&mut self) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        fn call(&mut self, name: &str, input: &Value) -> ToolResult {
            self.0.push((name.to_string(), input.clone()));
            ToolResult::ok("=> 3")
        }
    }

    fn text_reply(text: &str) -> Vec<Value> {
        vec![
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        ]
    }

    fn tool_reply(id: &str, json_input: &str, stop_reason: &str) -> Vec<Value> {
        vec![
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": id, "name": "eval", "input": {}}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": json_input}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": stop_reason}}),
        ]
    }

    fn config() -> TurnConfig {
        TurnConfig {
            model: "claude-opus-5-5".into(),
            effort: "high".into(),
            max_tokens: 64_000,
            max_rounds: 4,
            system: "sys".into(),
            tools: vec![json!({"name": "eval"})],
            dir: PathBuf::new(),
        }
    }

    fn run(script: Vec<Vec<Value>>) -> (Vec<Value>, Transcript, Vec<(String, Value)>) {
        run_with(&config(), script)
    }

    fn run_with(config: &TurnConfig, script: Vec<Vec<Value>>) -> (Vec<Value>, Transcript, Vec<(String, Value)>) {
        let transport = Script(Mutex::new(script));
        let mut tools = Tools(Vec::new());
        let mut history = Vec::new();
        push_user_text(&mut history, "add 1 and 2");
        let transcript = Mutex::new(Transcript::default());
        run_turn(&transport, &mut tools, config, &mut history, &transcript, &AtomicBool::new(false)).unwrap();
        (history, transcript.into_inner().unwrap(), tools.0)
    }

    #[test]
    fn a_tool_call_runs_and_its_result_goes_back_before_the_final_answer() {
        let (history, transcript, calls) = run(vec![tool_reply("t1", "{\"code\": \"(+ 1 2)\"}", "tool_use"), text_reply("It is 3.")]);
        assert_eq!(calls, vec![("eval".to_string(), json!({"code": "(+ 1 2)"}))]);
        let roles: Vec<&str> = history.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        assert_eq!(history[2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(history[2]["content"][0]["is_error"], false);
        assert!(matches!(&transcript.entries[0], Entry::Tool { status: ToolStatus::Done, output, .. } if output == "=> 3"));
        assert_eq!(transcript.entries.last(), Some(&Entry::Assistant("It is 3.".into())));
    }

    #[test]
    fn the_conversation_is_recorded_each_round_without_image_data() {
        let recorded = TurnConfig {
            dir: std::env::temp_dir().join(format!("eseq-turn-record-test-{}", std::process::id())),
            ..config()
        };
        let (history, ..) =
            run_with(&recorded, vec![tool_reply("t1", "{\"code\": \"(+ 1 2)\"}", "tool_use"), text_reply("It is 3.")]);
        let path = recorded.dir.join("conversation.json");
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["model"], "claude-opus-5-5");
        assert_eq!(saved["messages"], json!(history));

        let image = json!({ "type": "image", "source": { "type": "base64", "data": "AAAA" } });
        record(&recorded, &[json!({ "role": "user", "content": [{ "type": "tool_result", "content": [image] }] })]);
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["messages"][0]["content"][0]["content"][0], json!({ "type": "image", "data": "omitted" }));
        let _ = std::fs::remove_dir_all(&recorded.dir);
    }

    #[test]
    fn invalid_or_truncated_tool_input_is_answered_without_running_the_tool() {
        let (history, _, calls) = run(vec![tool_reply("t1", "{\"code\": \"(+", "tool_use"), text_reply("Retrying.")]);
        assert!(calls.is_empty());
        assert_eq!(history[2]["content"][0]["is_error"], true);
        assert!(history[2]["content"][0]["content"][0]["text"].as_str().unwrap().contains("INVALID_JSON"));

        let (history, transcript, calls) = run(vec![tool_reply("t2", "{\"code\": \"(+ 1 2)\"}", "max_tokens")]);
        assert!(calls.is_empty(), "a max_tokens turn's tools never run");
        assert_eq!(history.last().unwrap()["content"][0]["tool_use_id"], "t2", "the history stays valid");
        assert_eq!(transcript.entries.last(), Some(&Entry::Notice("The reply reached its length limit.".into())));
    }

    #[test]
    fn a_refusal_keeps_only_text_and_a_cut_off_stream_is_an_error_with_its_tools_unrun() {
        let mut refused = tool_reply("t1", "{}", "refusal");
        refused.insert(0, json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "Sure"}}));
        let (history, transcript, calls) = run(vec![refused]);
        assert!(calls.is_empty());
        assert_eq!(history.last().unwrap()["content"], json!([{"type": "text", "text": "Sure"}]), "no unanswered tool_use");
        assert!(transcript.entries.iter().any(|e| matches!(e, Entry::Tool { status: ToolStatus::Failed, .. })));

        let transport = Script(Mutex::new(vec![tool_reply("t2", "{}", "tool_use")[..3].to_vec()]));
        let mut history = Vec::new();
        push_user_text(&mut history, "hi");
        let transcript = Mutex::new(Transcript::default());
        let ended =
            run_turn(&transport, &mut Tools(Vec::new()), &config(), &mut history, &transcript, &AtomicBool::new(false));
        assert_eq!(ended, Err("The response ended before the message finished.".to_string()));
        assert_eq!(history.len(), 1, "nothing of the cut-off message went back");
        assert!(matches!(&transcript.into_inner().unwrap().entries[0], Entry::Tool { status: ToolStatus::Failed, .. }));
    }

    #[test]
    fn a_new_user_message_joins_a_user_turn_already_open() {
        let mut history = vec![json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t"}]})];
        push_user_text(&mut history, "go on");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["content"][1], json!({"type": "text", "text": "go on"}));
        push_user_text(&mut Vec::new(), "x");
    }

    #[test]
    fn the_request_streams_caches_and_falls_back() {
        let body: Value =
            serde_json::from_slice(&request_body(&config(), &[json!({"role": "user", "content": "hi"})])).unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["system"][0]["text"], "sys");
        assert_eq!(body["tools"][0]["name"], "eval");
        assert_eq!(body["messages"][0]["content"], "hi");
        let haiku = TurnConfig { model: "claude-haiku-5-5".into(), ..config() };
        let body: Value = serde_json::from_slice(&request_body(&haiku, &[])).unwrap();
        assert!(body.get("fallbacks").is_none(), "only Opus has fallback models");
    }
}
