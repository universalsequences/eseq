//! The agent's tools: a `metal_seq mcp` child process, spoken to over
//! JSON-RPC on its stdio. The in-app agent gets exactly the tools any MCP
//! client gets, against its own headless eseq, so it never touches the live
//! session; what it writes (Local modules) reaches the app by hot reload.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::lock;

use serde_json::{json, Value};

/// A tool call's result as Messages API `tool_result` content.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
    /// The PNGs among `content`, decoded, for the transcript.
    pub images: Vec<Vec<u8>>,
}

impl ToolResult {
    pub fn error(text: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": text.into()})], is_error: true, images: Vec::new() }
    }

    /// The result's text, joined.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where tools come from: the MCP child, or a fake in tests.
pub trait ToolBackend: Send {
    /// Messages API tool definitions.
    fn tools(&mut self) -> Result<Vec<Value>, String>;
    fn call(&mut self, name: &str, input: &Value) -> ToolResult;
}

/// The MCP revision the client asks for; the server lists it first.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// The tool server process, shared so another thread can interrupt a call
/// that does not come back.
#[derive(Clone)]
pub struct ServerHandle {
    child: Arc<Mutex<Child>>,
    in_call: Arc<AtomicBool>,
}

impl ServerHandle {
    /// End the server if a call is waiting on it (its read then sees EOF);
    /// the next turn starts a new one.
    pub fn interrupt(&self) {
        if self.in_call.load(Ordering::Relaxed) {
            let _ = lock(&self.child).kill();
        }
    }

    fn alive(&self) -> bool {
        matches!(lock(&self.child).try_wait(), Ok(None))
    }
}

pub struct McpClient {
    server: ServerHandle,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// The server's `instructions`: how to use its tools.
    pub instructions: String,
}

impl McpClient {
    /// Start `program args…` as an MCP server (its stderr to `log`) and
    /// complete the handshake.
    pub fn spawn(program: &Path, args: &[String], log: &Path) -> Result<Self, String> {
        let stderr = std::fs::File::create(log).map_err(|error| format!("cannot write {}: {error}", log.display()))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
        let stdin = child.stdin.take().ok_or("the tool server has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the tool server has no stdout")?);
        let server = ServerHandle { child: Arc::new(Mutex::new(child)), in_call: Arc::new(AtomicBool::new(false)) };
        let mut client = Self { server, stdin, stdout, next_id: 1, instructions: String::new() };
        let init = client.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "eseq-agent", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        client.instructions = init["instructions"].as_str().unwrap_or_default().to_string();
        client.notify("notifications/initialized")?;
        Ok(client)
    }

    pub fn handle(&self) -> ServerHandle {
        self.server.clone()
    }

    /// Whether the server is still running (a crash or an interrupt ends it).
    pub fn alive(&self) -> bool {
        self.server.alive()
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        writeln!(self.stdin, "{message}").and_then(|()| self.stdin.flush()).map_err(|error| error.to_string())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.server.in_call.store(true, Ordering::Relaxed);
        let response = self.exchange(method, params);
        self.server.in_call.store(false, Ordering::Relaxed);
        response
    }

    fn exchange(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{message}")
            .and_then(|()| self.stdin.flush())
            .map_err(|error| format!("the tool server stopped: {error}"))?;
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).map_err(|error| error.to_string())?;
            if read == 0 {
                return Err("The tool server exited; the next turn starts a new one.".to_string());
            }
            let Ok(response) = serde_json::from_str::<Value>(&line) else { continue };
            if response["id"] != json!(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(error["message"].as_str().unwrap_or("tool server error").to_string());
            }
            return Ok(response["result"].clone());
        }
    }
}

/// Ends the server when the client goes. The app's session is a static, so
/// at app exit this never runs: the server ends on its own when its stdin
/// closes.
impl Drop for McpClient {
    fn drop(&mut self) {
        let mut child = lock(&self.server.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl ToolBackend for McpClient {
    fn tools(&mut self) -> Result<Vec<Value>, String> {
        let listed = self.request("tools/list", json!({}))?;
        Ok(listed["tools"].as_array().map(|tools| tools.iter().map(mcp_tool_to_api).collect()).unwrap_or_default())
    }

    fn call(&mut self, name: &str, input: &Value) -> ToolResult {
        match self.request("tools/call", json!({ "name": name, "arguments": input })) {
            Ok(result) => mcp_result_to_tool_result(&result),
            Err(error) => ToolResult::error(error),
        }
    }
}

/// An MCP tool as a Messages API tool. Requests stream, so each tool streams
/// its input as it is written (and the harness validates it).
pub fn mcp_tool_to_api(tool: &Value) -> Value {
    json!({
        "name": tool["name"],
        "description": tool["description"],
        "input_schema": tool["inputSchema"],
        "eager_input_streaming": true,
    })
}

/// MCP `tools/call` content as `tool_result` content: text stays text, an
/// image becomes a base64 image block.
pub fn mcp_result_to_tool_result(result: &Value) -> ToolResult {
    use base64::Engine as _;
    let mut images = Vec::new();
    let content = result["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item["type"].as_str() {
                    Some("text") => Some(json!({ "type": "text", "text": item["text"] })),
                    Some("image") => {
                        let data = item["data"].as_str().unwrap_or_default();
                        if let Ok(png) = base64::engine::general_purpose::STANDARD.decode(data) {
                            images.push(png);
                        }
                        Some(json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": item["mimeType"], "data": data },
                        }))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    ToolResult { content, is_error: result["isError"].as_bool().unwrap_or(false), images }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tools_and_results_map_to_messages_api_shapes() {
        let tool = json!({"name": "eval", "description": "Run code.", "inputSchema": {"type": "object"}});
        assert_eq!(
            mcp_tool_to_api(&tool),
            json!({"name": "eval", "description": "Run code.", "input_schema": {"type": "object"}, "eager_input_streaming": true})
        );
        let result = mcp_result_to_tool_result(&json!({
            "content": [
                {"type": "image", "mimeType": "image/png", "data": "iVBO"},
                {"type": "text", "text": "a report"},
            ],
            "isError": false,
        }));
        assert_eq!(
            result.content,
            vec![
                json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBO"}}),
                json!({"type": "text", "text": "a report"}),
            ]
        );
        assert_eq!((result.is_error, result.images.len(), result.text().as_str()), (false, 1, "a report"));
    }
}
