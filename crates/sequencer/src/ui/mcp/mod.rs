//! `metal_seq mcp`: eseq's agent tools over the Model Context Protocol, for
//! any MCP client (Claude Code, Codex, …) and as the test bench for the
//! in-app agent, which calls the same tools (bead eseq-maxc.7).
//!
//! One headless app (the `capture` boot: engine, project, UI runtime,
//! editor) lives for the whole session, so definitions, instances and
//! buffers persist between calls. The transport is JSON-RPC 2.0 over stdio,
//! one message per line; only `initialize`, `ping`, `tools/list` and
//! `tools/call` are served.

#[cfg(target_os = "macos")]
mod stdio;
#[cfg(target_os = "macos")]
mod tools;

use std::path::PathBuf;

pub(crate) struct McpArgs {
    project: Option<PathBuf>,
}

impl McpArgs {
    pub(crate) fn parse_env() -> Result<Option<Self>, String> {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() != Some("mcp") {
            return Ok(None);
        }
        let mut project = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--project" => {
                    let path = args.next().ok_or("--project requires a saved project path")?;
                    project = Some(std::path::absolute(path).map_err(|error| error.to_string())?);
                }
                _ => return Err("usage: metal_seq mcp [--project SAVED_PROJECT_JSON]".to_string()),
            }
        }
        Ok(Some(Self { project }))
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn run(_args: McpArgs) -> Result<(), Box<dyn std::error::Error>> {
    Err("metal_seq mcp needs the Metal capture path and is macOS-only for now".into())
}

#[cfg(target_os = "macos")]
pub(crate) use server::run;

#[cfg(target_os = "macos")]
mod server {
    use std::io::{BufRead, BufWriter, Write};

    use serde_json::{json, Value as Json};

    use super::super::capture::{parse_capture_source, HeadlessSession};
    use super::{stdio, tools, McpArgs};

    const PROTOCOL_VERSIONS: &[&str] = &[sequencer::agent::harness::mcp_client::MCP_PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

    const INSTRUCTIONS: &str = "These tools run against a live, headless eseq: a music \
sequencer whose UI, sequencers and extensions are written in eseqlisp, a small Lisp. The \
session keeps its state between calls.\n\
\n\
Before writing code, read `guide` (topic `sequencer` for sequencers, `eseqlisp` for the \
language). Never guess a name: find it with `apropos`, read it with `describe`, and read a \
module with `outline`, then `form_source` for the forms you need.\n\
\n\
Your work lives on disk, in a Local module: start it with `create_module`, then add or \
replace one top-level form at a time with `define_form`, which writes the file and reloads \
the module (a failed reload restores the file and returns the errors). `eval` is for \
questions and experiments; nothing it defines is saved, and `write_module` replaces a whole \
file.\n\
\n\
Check your work here rather than as the guides describe for the command line: `new_instance` \
makes an instance of your kind; `simulate` runs its tick for a few bars and lists every hit \
(instead of asking the user to press Play); `capture` draws its panel and reports clipped, \
offscreen and overlapping widgets (instead of `eseq sequencer check`), and with `click` and \
`keys` it operates the panel so you can confirm an edit changed the document. Fix what they \
report before calling the work done.";

    pub(crate) fn run(args: McpArgs) -> Result<(), Box<dyn std::error::Error>> {
        let mut out = BufWriter::new(stdio::take_stdout()?);
        let (setup, project) = match &args.project {
            Some(path) => ("(capture-project)\n", format!("The session opened the project {}.", path.display())),
            None => (
                sequencer::authoring_kit::SEQUENCER_CHECK_PROJECT,
                "The session starts with four empty sampler tracks: Kick, Snare, Hat, Perc.".to_string(),
            ),
        };
        let instructions = format!("{project}\n\n{INSTRUCTIONS}");
        // Boot evaluates the setup source as if it lived at this path; the
        // file itself is never read.
        let script = std::env::temp_dir().join("eseq-mcp-setup.lisp");
        let parsed = parse_capture_source(setup)?;
        let mut session = HeadlessSession::boot(&script, &parsed, args.project.as_deref(), 0, false)?;
        tools::load_lisp_tools(&mut session);
        eprintln!("eseq mcp: ready");

        for line in std::io::stdin().lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Json>(&line) {
                // A batch (protocol 2025-03-26) answers with an array of the
                // requests' responses.
                Ok(Json::Array(batch)) => {
                    let responses: Vec<Json> = batch
                        .iter()
                        .filter_map(|message| handle(&mut session, &instructions, message))
                        .collect();
                    (!responses.is_empty()).then(|| Json::Array(responses))
                }
                Ok(message) => handle(&mut session, &instructions, &message),
                Err(error) => Some(error_response(Json::Null, -32700, &format!("parse error: {error}"))),
            };
            if let Some(response) = response {
                serde_json::to_writer(&mut out, &response)?;
                out.write_all(b"\n")?;
                out.flush()?;
            }
        }
        Ok(())
    }

    /// The response to one message, or None for a notification.
    fn handle(session: &mut HeadlessSession, instructions: &str, message: &Json) -> Option<Json> {
        let id = message.get("id")?.clone();
        let Some(method) = message.get("method").and_then(Json::as_str) else {
            return Some(error_response(id, -32600, "invalid request: no method"));
        };
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        let result = match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Json::as_str);
                let version = requested
                    .filter(|version| PROTOCOL_VERSIONS.contains(version))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "eseq", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": instructions,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({ "tools": tools::list(session) }),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Json::as_str) else {
                    return Some(error_response(id, -32602, "tools/call needs a tool name"));
                };
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                tools::call(session, name, &arguments).into_json()
            }
            _ => return Some(error_response(id, -32601, &format!("method not found: {method}"))),
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn error_response(id: Json, code: i64, message: &str) -> Json {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
    }
}
