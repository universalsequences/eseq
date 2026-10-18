//! The agent pane's natives (`agent-pane/…`) and its per-frame publish. The
//! conversation itself is a `sequencer::agent::harness::AgentSession`, made
//! on first use; its turns run on their own thread, so the event loop only
//! compares the transcript's generation each frame and, when it moved,
//! presents it as `agent.pane-generation` for the pane to redraw.

use std::sync::OnceLock;

use eseqlisp::vm::Value;
use eseqlisp::{Editor, Runtime};
use eseqlisp::package::PackageCatalog;
use std::collections::HashSet;
use std::rc::Rc;

use sequencer::agent::harness::{clip, AgentSession, Artifact, Entry, SessionConfig, Transcript, EFFORTS, MODELS};

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

fn clipped(body: &str, limit: usize) -> Value {
    Value::String(clip(body, limit))
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

/// Whether the project imports any module of `package` (from its scratch
/// or init.lisp): then it must not be renamed or deleted under it.
fn in_use(package: &str, loaded: &HashSet<String>) -> bool {
    let prefix = format!("{package}.");
    loaded.iter().any(|module| module.starts_with(&prefix))
}

/// What the project imports, from its scratch and init.lisp.
fn loaded_modules(scratch: &str) -> HashSet<String> {
    let init = std::fs::read_to_string(crate::host_commands::packages::user_init_path()).unwrap_or_default();
    let mut loaded = sequencer::app::import_modules(scratch);
    loaded.extend(sequencer::app::import_modules(&init));
    loaded
}

fn artifact_value(artifact: &Artifact, loaded: &HashSet<String>, catalog: &PackageCatalog) -> Value {
    let files = artifact.files.iter().map(|file| {
        map_value([
            ("name", text(&file.name)),
            ("path", text(&file.path.display().to_string())),
            ("change", text(file.change.as_str())),
            ("lines", Value::Number(file.lines as f64)),
            ("added", Value::Number(file.added as f64)),
            ("removed", Value::Number(file.removed as f64)),
        ])
    });
    let kinds = artifact
        .kinds
        .iter()
        .map(|decl| {
            // The registry's id for the kind, which creating an instance takes.
            let package = catalog.package_for_module(&decl.module).map(|package| package.manifest.name.as_str());
            let id = sequencer::lisp_host::kind_id(package, Some(&decl.module), &decl.kind);
            map_value([("module", text(&decl.module)), ("kind", text(&decl.kind)), ("id", text(&id))])
        });
    map_value([
        ("package", text(&artifact.package)),
        ("created", Value::Bool(artifact.created)),
        ("loaded", Value::Bool(in_use(&artifact.package, loaded))),
        ("lines", Value::Number(artifact.lines() as f64)),
        ("added", Value::Number(artifact.added() as f64)),
        ("removed", Value::Number(artifact.removed() as f64)),
        ("files", list_value(files)),
        ("entries", list_value(artifact.entries.iter().map(|module| text(module)))),
        ("kinds", list_value(kinds)),
    ])
}

fn usage_value(transcript: &Transcript) -> Value {
    map_value([
        ("input", Value::Number(transcript.input_tokens as f64)),
        ("output", Value::Number(transcript.output_tokens as f64)),
        ("cache-read", Value::Number(transcript.cache_read_tokens as f64)),
    ])
}

/// The pane's whole view of the conversation, read under one lock;
/// `loaded` is the modules the project imports.
fn state_value(transcript: &Transcript, loaded: &HashSet<String>) -> Value {
    // Read once: the catalog checks the package folders on every call.
    let catalog = if transcript.artifacts.is_empty() {
        Default::default()
    } else {
        sequencer::app_paths::app_paths().package_catalog()
    };
    map_value([
        ("busy", Value::Bool(transcript.busy)),
        ("entries", list_value(transcript.entries.iter().map(entry_value))),
        ("usage", usage_value(transcript)),
        ("artifacts", list_value(transcript.artifacts.iter().map(|artifact| artifact_value(artifact, loaded, &catalog)))),
    ])
}

fn string_arg(args: &[Value], index: usize, what: &str) -> Result<String, String> {
    match args.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(format!("expected {what}")),
    }
}

/// `scratch` reads the project's evaluated scratch, whose imports are what
/// is loaded.
pub(crate) fn register_agent_pane_natives(runtime: &mut Runtime, scratch: impl Fn() -> String + 'static) {
    register_choice_natives(runtime);
    let scratch = Rc::new(scratch);
    let (state_scratch, reject_scratch) = (Rc::clone(&scratch), Rc::clone(&scratch));
    runtime.register_native_with_docs(
        "agent-pane/send",
        "(agent-pane/send text)",
        "Send the user's message to the agent; its turn runs in the background.",
        |args, _ctx| {
            session()?.send(&string_arg(&args, 0, "the message text")?)?;
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
        "The conversation: (dict :busy :entries :usage :artifacts). Each entry has :kind \"user\", \"assistant\", \"thinking\", \"notice\" (and :text) or \"tool\" (:name :input :status :output :images); long text is clipped. :usage is (dict :input :output :cache-read) tokens. Each artifact is a package the agent wrote: :package :created :loaded :lines :added :removed :entries :kinds (dicts :module :kind :id, the id packages-new-instance takes) and :files (dicts :name :path :change :lines :added :removed). Read it under agent.pane-generation to redraw as it changes.",
        move |_args, _ctx| {
            let session = session()?;
            // Only cards show what is loaded; most turns have none.
            let loaded = if session.with_transcript(|transcript| transcript.artifacts.is_empty()) {
                HashSet::new()
            } else {
                loaded_modules(&state_scratch())
            };
            Ok(session.with_transcript(|transcript| state_value(transcript, &loaded)))
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/reject",
        "(agent-pane/reject package)",
        "Undo what this conversation wrote to a package: a package the agent made is deleted, an existing one gets its files back. Returns what happened, or why it did not, for the status line.",
        move |args, _ctx| {
            let package = string_arg(&args, 0, "a package name")?;
            let session = session()?;
            let created = session.with_transcript(|transcript| {
                transcript.artifacts.iter().any(|artifact| artifact.package == package && artifact.created)
            });
            if created && in_use(&package, &loaded_modules(&reject_scratch())) {
                return Ok(text("Unload the package before rejecting it."));
            }
            Ok(text(&session.reject(&package).unwrap_or_else(|why| why)))
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/rename",
        "(agent-pane/rename package new-name)",
        "Rename a package the agent made: its folder and its module names. Returns what happened, or why it did not.",
        move |args, _ctx| {
            let (package, new_name) = (string_arg(&args, 0, "a package name")?, string_arg(&args, 1, "the new name")?);
            if in_use(&package, &loaded_modules(&scratch())) {
                return Ok(text("Unload the package to rename it."));
            }
            Ok(text(&session()?.rename(&package, &new_name).unwrap_or_else(|why| why)))
        },
    );
}

/// The composer's settings: the model and effort the next turn uses,
/// whether the conversation has started (for its placeholder), and its
/// token counts. Cheap, unlike `state_value`: the composer reads it as a
/// turn streams.
fn choice_value(session: &AgentSession) -> Value {
    let (model, effort) = session.choice();
    let label = MODELS.iter().find(|(id, _)| *id == model).map_or(model.as_str(), |(_, label)| label);
    map_value([
        ("model", text(&model)),
        ("label", text(label)),
        ("effort", text(&effort)),
        ("started", Value::Bool(session.with_transcript(|transcript| !transcript.entries.is_empty()))),
        ("busy", Value::Bool(session.busy())),
        ("usage", session.with_transcript(usage_value)),
    ])
}

fn register_choice_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "agent-pane/choice",
        "(agent-pane/choice)",
        "The composer's settings: (dict :model :label :effort :started :busy :usage), the model id and its name, the effort, whether the conversation has entries, whether a turn runs, and the tokens so far (as agent-pane/state's :usage).",
        |_args, _ctx| Ok(choice_value(session()?)),
    );
    runtime.register_native_with_docs(
        "agent-pane/models",
        "(agent-pane/models)",
        "The models the agent can use: dicts :id :label.",
        |_args, _ctx| {
            Ok(list_value(MODELS.iter().map(|(id, label)| map_value([("id", text(id)), ("label", text(label))]))))
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/efforts",
        "(agent-pane/efforts)",
        "The effort levels, least to most.",
        |_args, _ctx| Ok(list_value(EFFORTS.iter().map(|effort| text(effort)))),
    );
    runtime.register_native_with_docs(
        "agent-pane/set-model",
        "(agent-pane/set-model id)",
        "Use the model with this id from the next turn on.",
        |args, _ctx| {
            session()?.set_model(&string_arg(&args, 0, "a model id")?)?;
            Ok(Value::Nil)
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/set-effort",
        "(agent-pane/set-effort effort)",
        "Use this effort (low … max) from the next turn on.",
        |args, _ctx| {
            session()?.set_effort(&string_arg(&args, 0, "an effort level")?)?;
            Ok(Value::Nil)
        },
    );
}

/// Redraw the pane (when it is in use): the project's loaded packages changed.
pub(crate) fn refresh() {
    if let Some(Ok(session)) = SESSION.get() {
        session.touch();
    }
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
            artifacts: vec![Artifact {
                package: "ring".into(),
                created: true,
                files: Vec::new(),
                entries: vec!["ring.view".into()],
                kinds: vec![sequencer::agent::harness::KindDecl { module: "ring.view".into(), kind: "ring".into() }],
            }],
            ..Transcript::default()
        };
        let shown = Runtime::new().format_value(&state_value(&transcript, &HashSet::new()));
        for part in [
            ":busy true",
            ":kind \"user\"",
            ":kind \"tool\"",
            ":name \"capture\"",
            ":status \"done\"",
            ":images (\"/tmp/a.png\")",
            ":input 7",
            &format!(":input \"{}…\"", "x".repeat(INPUT_CHARS)),
            // Add <kind> creates an instance by the registry's id, not the bare name.
            ":id \"ring.view:ring\"",
            ":loaded false",
        ] {
            assert!(shown.contains(part), "{part} in {shown}");
        }
    }
}
