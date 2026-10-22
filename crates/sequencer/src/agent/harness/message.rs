//! One streamed assistant message, rebuilt from its events. Blocks are kept
//! as the API's own JSON, whatever their type (text, thinking with its
//! signature, tool_use, a refusal fallback's marker): the history sends them
//! back unchanged, which thinking blocks require.

use serde_json::{json, Value};

/// What an event changed, for the transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamUpdate {
    Text(String),
    Thinking(String),
    ToolStarted { id: String, name: String },
    Nothing,
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    /// A long server-side turn the API paused: send the history back to go on.
    PauseTurn,
    MaxTokens,
    Refusal,
    Other(String),
}

impl StopReason {
    fn parse(reason: &str) -> Self {
        match reason {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "pause_turn" => Self::PauseTurn,
            "max_tokens" => Self::MaxTokens,
            "refusal" => Self::Refusal,
            other => Self::Other(other.to_string()),
        }
    }
}

/// Blocks a mid-output fallback leaves behind: the history must omit those
/// that come before the last `fallback` block.
const DECLINED_BLOCK_TYPES: [&str; 4] = ["thinking", "redacted_thinking", "tool_use", "server_tool_use"];

/// A `tool_use` block of the finished message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// The streamed input when it was not one JSON object: the tool must
    /// not run on it (eager input streaming does not validate).
    pub invalid_input: Option<String>,
}

#[derive(Default)]
pub struct MessageAccumulator {
    blocks: Vec<Value>,
    partial_json: Vec<String>,
    invalid_input: Vec<Option<String>>,
    pub stop_reason: Option<StopReason>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
}

impl MessageAccumulator {
    pub fn apply(&mut self, event: &Value) -> Result<StreamUpdate, String> {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message_start" => {
                let usage = &event["message"]["usage"];
                self.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                self.cache_read_tokens = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_start" => {
                while self.blocks.len() <= index {
                    self.blocks.push(Value::Null);
                    self.partial_json.push(String::new());
                    self.invalid_input.push(None);
                }
                let block = event["content_block"].clone();
                self.blocks[index] = block.clone();
                if block["type"] == "tool_use" {
                    return Ok(StreamUpdate::ToolStarted {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                    });
                }
            }
            "content_block_delta" => {
                let delta = &event["delta"];
                let block = self.blocks.get_mut(index).ok_or("a delta for a block that never started")?;
                let append = |block: &mut Value, field: &str, text: &str| {
                    let previous = block[field].as_str().unwrap_or_default().to_string();
                    block[field] = Value::String(previous + text);
                };
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        append(block, "text", text);
                        return Ok(StreamUpdate::Text(text.to_string()));
                    }
                    "thinking_delta" => {
                        let text = delta["thinking"].as_str().unwrap_or_default();
                        append(block, "thinking", text);
                        return Ok(StreamUpdate::Thinking(text.to_string()));
                    }
                    "signature_delta" => append(block, "signature", delta["signature"].as_str().unwrap_or_default()),
                    "input_json_delta" => {
                        self.partial_json[index].push_str(delta["partial_json"].as_str().unwrap_or_default())
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.blocks.get_mut(index).filter(|block| block["type"] == "tool_use") {
                    let raw = std::mem::take(&mut self.partial_json[index]);
                    match serde_json::from_str::<Value>(if raw.trim().is_empty() { "{}" } else { &raw }) {
                        Ok(input @ Value::Object(_)) => block["input"] = input,
                        _ => {
                            block["input"] = json!({});
                            self.invalid_input[index] = Some(raw);
                        }
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(StopReason::parse(reason));
                }
                if let Some(tokens) = event["usage"]["output_tokens"].as_u64() {
                    self.output_tokens = tokens;
                }
            }
            "error" => {
                return Err(event["error"]["message"].as_str().unwrap_or("the API stream failed").to_string())
            }
            _ => {}
        }
        Ok(StreamUpdate::Nothing)
    }

    /// Where the blocks the model may still stand behind begin: after the
    /// last `fallback` block, whose declined attempt's thinking and tool
    /// calls must not go back.
    fn kept_from(&self) -> usize {
        self.blocks.iter().rposition(|block| block["type"] == "fallback").map_or(0, |index| index + 1)
    }

    /// The message's content blocks as the API sent them, for the history:
    /// without a declined attempt's thinking and tool calls, a thinking
    /// block cut off before its signature, or an empty text block (the API
    /// rejects both).
    pub fn content(&self) -> Vec<Value> {
        let kept_from = self.kept_from();
        self.blocks
            .iter()
            .enumerate()
            .filter(|(index, block)| {
                let kind = block["type"].as_str().unwrap_or_default();
                !block.is_null()
                    && (*index >= kept_from || !DECLINED_BLOCK_TYPES.contains(&kind))
                    && !(kind == "thinking" && block["signature"].as_str().unwrap_or_default().is_empty())
                    && !(kind == "text" && block["text"].as_str().unwrap_or_default().is_empty())
            })
            .map(|(_, block)| block.clone())
            .collect()
    }

    /// The calls to run: those after any fallback boundary.
    pub fn tool_uses(&self) -> Vec<ToolUse> {
        self.blocks
            .iter()
            .zip(&self.invalid_input)
            .skip(self.kept_from())
            .filter(|(block, _)| block["type"] == "tool_use")
            .map(|(block, invalid)| ToolUse {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                name: block["name"].as_str().unwrap_or_default().to_string(),
                input: block["input"].clone(),
                invalid_input: invalid.clone(),
            })
            .collect()
    }

    /// Only the text blocks: what may stand for a message cut off mid-way
    /// (a partial thinking block has no signature, a partial tool call no
    /// result).
    pub fn text_blocks(&self) -> Vec<Value> {
        self.blocks
            .iter()
            .filter(|block| block["type"] == "text" && !block["text"].as_str().unwrap_or_default().is_empty())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_all(events: &[Value]) -> (MessageAccumulator, Vec<StreamUpdate>) {
        let mut message = MessageAccumulator::default();
        let updates = events.iter().map(|event| message.apply(event).unwrap()).collect();
        (message, updates)
    }

    #[test]
    fn blocks_rebuild_with_thinking_signatures_text_and_parsed_tool_input() {
        let (message, updates) = apply_all(&[
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 10, "cache_read_input_tokens": 4}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Plan."}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Hel"}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "lo"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t1", "name": "eval", "input": {}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"code\": \"(+ "}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "1 2)\"}"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 30}}),
            json!({"type": "message_stop"}),
        ]);
        assert_eq!(
            message.content(),
            vec![
                json!({"type": "thinking", "thinking": "Plan.", "signature": "sig"}),
                json!({"type": "text", "text": "Hello"}),
                json!({"type": "tool_use", "id": "t1", "name": "eval", "input": {"code": "(+ 1 2)"}}),
            ]
        );
        assert_eq!(message.stop_reason, Some(StopReason::ToolUse));
        assert_eq!((message.input_tokens, message.output_tokens, message.cache_read_tokens), (10, 30, 4));
        assert!(updates.contains(&StreamUpdate::Text("lo".into())));
        assert!(updates.contains(&StreamUpdate::ToolStarted { id: "t1".into(), name: "eval".into() }));
        assert_eq!(message.tool_uses()[0].invalid_input, None);
    }

    #[test]
    fn a_tool_input_that_is_not_one_object_is_flagged_not_run() {
        let (message, _) = apply_all(&[
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t", "name": "eval", "input": {}}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"code\": \"(oops"}}),
            json!({"type": "content_block_stop", "index": 0}),
        ]);
        let tool = &message.tool_uses()[0];
        assert_eq!(tool.invalid_input.as_deref(), Some("{\"code\": \"(oops"));
        assert_eq!(tool.input, json!({}), "the history still holds a valid block");
    }

    #[test]
    fn a_declined_attempts_thinking_and_calls_stay_out_of_the_history_after_a_fallback() {
        let (message, _) = apply_all(&[
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": "a"}}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "Part"}}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "old", "name": "eval", "input": {}}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "fallback", "from": {}, "to": {}}}),
            json!({"type": "content_block_start", "index": 4, "content_block": {"type": "thinking", "thinking": "cut"}}),
            json!({"type": "content_block_start", "index": 5, "content_block": {"type": "tool_use", "id": "new", "name": "eval", "input": {}}}),
            json!({"type": "content_block_stop", "index": 5}),
        ]);
        let kinds: Vec<String> = message.content().iter().map(|b| b["type"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds, ["text", "fallback", "tool_use"], "no declined thinking or call, no unsigned thinking");
        let ids: Vec<String> = message.tool_uses().into_iter().map(|tool| tool.id).collect();
        assert_eq!(ids, ["new"]);
    }

    #[test]
    fn an_error_event_fails_the_stream() {
        let mut message = MessageAccumulator::default();
        let error = message.apply(&json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}));
        assert_eq!(error, Err("Overloaded".to_string()));
    }
}
