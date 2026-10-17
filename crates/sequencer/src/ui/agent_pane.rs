//! The agent pane's natives (`agent-pane/…`) and its per-frame publish. The
//! conversation itself is a `sequencer::agent::harness::AgentSession`, made
//! on first use; its turns run on their own thread, so the event loop only
//! compares the transcript's generation each frame and, when it moved,
//! presents it as `agent.pane-generation` for the pane to redraw.

use std::sync::OnceLock;

use eseqlisp::vm::Value;
use eseqlisp::{Editor, Runtime};
use sequencer::agent::harness::{AgentSession, Entry, SessionConfig, Transcript};

use super::presented::present_agent_pane;
use super::values::{list_value, map_value};

static SESSION: OnceLock<Result<AgentSession, String>> = OnceLock::new();

/// How much of a tool's input, output or the model's reasoning the pane
/// shows; the rest stays in the conversation.
const INPUT_CHARS: usize = 90;
const OUTPUT_CHARS: usize = 600;
const THINKING_CHARS: usize = 400;

fn session() -> Result<&'static AgentSession, String> {
    SESSION
        .get_or_init(|| {
            let state_dir = sequencer::app_paths::app_paths().user_data_root().join("agent");
            SessionConfig::for_this_app(state_dir).map(AgentSession::new)
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn text(value: &str) -> Value {
    Value::String(value.to_string())
}

/// `body` cut to `limit` characters, marked when cut.
fn clipped(body: &str, limit: usize) -> Value {
    match body.char_indices().nth(limit) {
        Some((end, _)) => Value::String(format!("{}…", &body[..end])),
        None => text(body),
    }
}

fn entry_value(entry: &Entry) -> Value {
    match entry {
        Entry::User(body) => map_value([("kind", text("user")), ("text", text(body))]),
        Entry::Assistant(body) => map_value([("kind", text("assistant")), ("text", text(body))]),
        Entry::Thinking(body) => map_value([("kind", text("thinking")), ("text", clipped(body, THINKING_CHARS))]),
        Entry::Notice(body) => map_value([("kind", text("notice")), ("text", text(body))]),
        Entry::Tool { name, input, status, output, images, .. } => map_value([
            ("kind", text("tool")),
            ("name", text(name)),
            ("input", clipped(input, INPUT_CHARS)),
            ("status", text(status.as_str())),
            ("output", clipped(output, OUTPUT_CHARS)),
            ("images", list_value(images.iter().map(|path| text(&path.display().to_string())))),
        ]),
    }
}

fn usage_value(transcript: &Transcript) -> Value {
    map_value([
        ("input", Value::Number(transcript.input_tokens as f64)),
        ("output", Value::Number(transcript.output_tokens as f64)),
        ("cache-read", Value::Number(transcript.cache_read_tokens as f64)),
    ])
}

/// The pane's whole view of the conversation, read under one lock.
fn state_value(transcript: &Transcript) -> Value {
    map_value([
        ("busy", Value::Bool(transcript.busy)),
        ("entries", list_value(transcript.entries.iter().map(entry_value))),
        ("usage", usage_value(transcript)),
    ])
}

pub(crate) fn register_agent_pane_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "agent-pane/send",
        "(agent-pane/send text)",
        "Send the user's message to the agent; its turn runs in the background.",
        |args, _ctx| {
            let Some(Value::String(message)) = args.first() else {
                return Err("agent-pane/send takes the message text".to_string());
            };
            session()?.send(message)?;
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/cancel",
        "(agent-pane/cancel)",
        "Stop the agent's running turn.",
        |_args, _ctx| {
            session()?.cancel();
            Ok(Value::Nil)
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/reset",
        "(agent-pane/reset)",
        "Start a new conversation with the agent.",
        |_args, _ctx| {
            session()?.reset()?;
            Ok(Value::Nil)
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/busy?",
        "(agent-pane/busy?)",
        "Whether the agent is working on a turn.",
        |_args, _ctx| Ok(Value::Bool(session().is_ok_and(AgentSession::busy))),
    );
    runtime.register_native_with_docs(
        "agent-pane/state",
        "(agent-pane/state)",
        "The conversation: (dict :busy :entries :usage). Each entry has :kind \"user\", \"assistant\", \"thinking\", \"notice\" (and :text) or \"tool\" (:name :input :status :output :images); long text is clipped. :usage is (dict :input :output :cache-read) tokens. Read it under agent.pane-generation to redraw as it changes.",
        |_args, _ctx| Ok(session()?.with_transcript(state_value)),
    );
}

/// Once per frame: when the conversation changed since `seen`, present the
/// new generation so the pane redraws. Nothing happens until the pane is
/// first used.
pub(crate) fn publish(editor: &mut Editor, seen: &mut u64) {
    let Some(Ok(session)) = SESSION.get() else { return };
    let generation = session.generation();
    if *seen == generation {
        return;
    }
    *seen = generation;
    present_agent_pane(generation);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.refresh_visible_layouts_for_buffer_named("*agent-pane*");
    editor.mark_needs_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_entries_become_lisp_dicts_the_pane_reads() {
        let transcript = Transcript {
            entries: vec![
                Entry::User("hi".into()),
                Entry::Tool {
                    id: "t1".into(),
                    name: "capture".into(),
                    input: "x".repeat(INPUT_CHARS + 5),
                    status: sequencer::agent::harness::ToolStatus::Done,
                    output: "ok".into(),
                    images: vec!["/tmp/a.png".into()],
                },
            ],
            busy: true,
            input_tokens: 7,
            ..Transcript::default()
        };
        let shown = Runtime::new().format_value(&state_value(&transcript));
        for part in [
            ":busy true",
            ":kind \"user\"",
            ":kind \"tool\"",
            ":name \"capture\"",
            ":status \"done\"",
            ":images (\"/tmp/a.png\")",
            ":input 7",
            &format!(":input \"{}…\"", "x".repeat(INPUT_CHARS)),
        ] {
            assert!(shown.contains(part), "{part} in {shown}");
        }
    }
}
