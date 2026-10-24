//! The agent pane's natives (`agent-pane/…`) and its per-frame publish. The
//! conversation itself is a `sequencer::agent::harness::AgentSession`, made
//! on first use; its turns run on their own thread, so the event loop only
//! compares the transcript's generation each frame and, when it moved,
//! presents it as `agent.pane-generation` for the pane to redraw. The
//! agent's `try` calls are served here too, on the UI thread ([`tried`]).

#[cfg(target_os = "macos")]
pub(crate) mod bench;
mod context;
mod dsp_edit;
pub(crate) use context::{dgen_values, view_state, HostView};
#[cfg(target_os = "macos")]
mod live_view;
mod tried;

use std::sync::OnceLock;

use eseqlisp::vm::Value;
use eseqlisp::{Editor, Runtime};
use eseqlisp::package::PackageCatalog;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use sequencer::agent::harness::{attachments, clip, AgentSession, Artifact, Entry, SessionConfig, Transcript, EFFORTS, MODELS};

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

fn paths_value(paths: &[PathBuf]) -> Value {
    list_value(paths.iter().map(|path| text(&path.display().to_string())))
}

fn entry_value(entry: &Entry) -> Value {
    match entry {
        Entry::User { text: body, images } => {
            map_value([("kind", text("user")), ("text", text(body)), ("images", paths_value(images))])
        }
        Entry::Assistant(body) => map_value([("kind", text("assistant")), ("text", text(body))]),
        Entry::Thinking(body) => map_value([("kind", text("thinking")), ("text", clipped(body, THINKING_CHARS))]),
        Entry::Notice(body) => map_value([("kind", text("notice")), ("text", text(body))]),
        Entry::Tool { name, input, status, output, images, .. } => map_value([
            ("kind", text("tool")),
            ("name", text(name)),
            ("input", clipped(input, INPUT_CHARS)),
            ("status", text(status.as_str())),
            ("output", clipped(output, OUTPUT_CHARS)),
            ("images", paths_value(images)),
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
        "agent-pane/attach",
        "(agent-pane/attach path)",
        "Attach the image file at path (PNG or JPEG) to the next message, as dropping it on the window does. It shows in agent-pane/attachments once copied in; one that cannot be attached becomes a notice in the conversation.",
        |args, _ctx| {
            session()?.attach(vec![PathBuf::from(string_arg(&args, 0, "an image path")?)]);
            Ok(Value::Nil)
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/attachments",
        "(agent-pane/attachments)",
        "The images attached to the next message, as file paths (staged copies), in order. Read it under agent.pane-shape.",
        |_args, _ctx| Ok(session()?.with_transcript(|transcript| paths_value(&transcript.attachments))),
    );
    runtime.register_native_with_docs(
        "agent-pane/detach",
        "(agent-pane/detach index)",
        "Take the image at index (as agent-pane/attachments lists them) off the next message.",
        |args, _ctx| match args.first() {
            Some(Value::Number(index)) if *index >= 0.0 => Ok(Value::Bool(session()?.detach(*index as usize))),
            _ => Err("expected an attachment index".to_string()),
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
        "The conversation: (dict :busy :entries :usage :artifacts). Each entry has :kind \"user\" (:text, and :images, the paths of the images sent with it), \"assistant\", \"thinking\", \"notice\" (and :text) or \"tool\" (:name :input :status :output :images); long text is clipped. :usage is (dict :input :output :cache-read) tokens. Each artifact is a package the agent wrote: :package :created :loaded :lines :added :removed :entries :kinds (dicts :module :kind :id, the id packages-new-instance takes) and :files (dicts :name :path :change :lines :added :removed). Read it under agent.pane-shape to redraw as rows change (agent.pane-generation also moves as text streams; read the last rows with agent-pane/entries-from under it).",
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
        "agent-pane/show-saved",
        "(agent-pane/show-saved path)",
        "Show a saved conversation (a conversation.json under .local/agent/conversations) in the pane, to read it back. Only shown: the next message starts a new conversation.",
        |args, _ctx| {
            session()?.show_saved(Path::new(&string_arg(&args, 0, "a conversation.json path")?))?;
            Ok(Value::Nil)
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/tried",
        "(agent-pane/tried)",
        "What the agent is trying in the app (its `try` tool), one dict per form, imports left out: :label (\"override eseq.mixer/strip-buttons\") and :open (its code shows in a tile beside the pane); empty when nothing is. The host command agent-pane-view-tried (dict :label …) opens or closes that tile; Keep and Revert are agent-pane-keep-tried and agent-pane-revert-tried.",
        |_args, _ctx| {
            Ok(list_value(
                tried::shown_forms()
                    .iter()
                    .map(|(label, open)| map_value([("label", text(label)), ("open", Value::Bool(*open))])),
            ))
        },
    );
    runtime.register_native_with_docs(
        "agent-pane/entries-from",
        "(agent-pane/entries-from start)",
        "The conversation's entries from index `start` on, as in agent-pane/state: for the last rows, which redraw under agent.pane-generation as text streams while the rest wait for agent.pane-shape.",
        |args, _ctx| {
            let start = match args.first() {
                Some(Value::Number(start)) if *start >= 0.0 => *start as usize,
                _ => return Err("expected an entry index".to_string()),
            };
            Ok(session()?.with_transcript(|transcript| {
                list_value(transcript.entries.iter().skip(start).map(entry_value))
            }))
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

/// The agent pane's buffer.
const PANE_BUFFER: &str = "*agent-pane*";

const SEND_COMMAND: &str = "agent-pane-send";
const ASK_COMMAND: &str = "agent-pane-ask";
const KEEP_COMMAND: &str = "agent-pane-keep-tried";
const REVERT_COMMAND: &str = "agent-pane-revert-tried";
const VIEW_COMMAND: &str = "agent-pane-view-tried";
/// The pane's host commands on what the agent is trying: Keep, Revert, and
/// show or hide one form's code.
/// The module what the agent tries runs as.
pub(crate) const TRY_MODULE: &str = tried::MODULE;

/// The source of what the agent is trying in the app, after its module line.
pub(crate) fn tried_source() -> String {
    tried::tried()
}

pub(crate) const COMMANDS: &[&str] = &[SEND_COMMAND, ASK_COMMAND, KEEP_COMMAND, REVERT_COMMAND, VIEW_COMMAND];

/// The composer's text box, which Ask focuses.
const INPUT_KEY: &str = "agent-pane-input";
/// Frames Ask keeps trying to focus the composer while the pane lays out.
const FOCUS_FRAMES: u8 = 30;

thread_local! {
    /// Frames left to focus the composer after Ask (0: none pending).
    static FOCUS_PENDING: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// Ask: open the pane on what the user is in now and put the caret in the
/// composer. The message's context names that tile, cursor and selection.
fn ask(editor: &mut Editor) -> Result<(), String> {
    context::remember_focus(editor);
    if !editor.buffer_named_is_showing(PANE_BUFFER) {
        editor
            .runtime_mut()
            .eval_str("(eseq.agent-pane/agent-pane-open)")
            .map_err(|error| format!("The agent pane did not open: {error:?}"))?;
        editor.refresh_runtime_side_effects();
    }
    editor.switch_active_tile_to_buffer_named(PANE_BUFFER);
    FOCUS_PENDING.with(|pending| pending.set(FOCUS_FRAMES));
    focus_composer(editor);
    Ok(())
}

/// Focus the composer if Ask is waiting for it and it is laid out.
fn focus_composer(editor: &mut Editor) {
    let left = FOCUS_PENDING.with(std::cell::Cell::get);
    if left == 0 {
        return;
    }
    let focused = editor.focus_widget_by_stable_key(INPUT_KEY, Some("textbox"));
    FOCUS_PENDING.with(|pending| pending.set(if focused { 0 } else { left - 1 }));
    if focused {
        editor.mark_needs_redraw();
    }
}

/// `host_views` are the views the host owns that the window may show (the
/// open instrument or effect editor), for a message's context.
pub(crate) fn handle_command(name: &str, payload: &Value, editor: &mut Editor, host_views: &[HostView]) {
    if name == ASK_COMMAND {
        if let Err(why) = ask(editor) {
            editor.show_transient_message(why);
        }
        return;
    }
    let session = match session() {
        Ok(session) => session,
        Err(why) => return editor.show_transient_message(format!("The agent pane is unavailable: {why}")),
    };
    let result = match name {
        SEND_COMMAND => {
            // What the window shows now, as the user sends.
            let text = match payload {
                Value::Map(map) => crate::history_commands::map_string(map, "text"),
                _ => None,
            };
            if let Err(why) = session.send_with_context(&text.unwrap_or_default(), context::header(editor, host_views)) {
                editor.show_transient_message(why);
            }
            return;
        }
        KEEP_COMMAND => tried::keep_in_init(editor),
        REVERT_COMMAND => tried::revert_tried(editor),
        VIEW_COMMAND => {
            let label = match payload {
                Value::Map(map) => crate::history_commands::map_string(map, "label"),
                _ => None,
            };
            if let Err(why) = label.ok_or_else(|| "which form?".to_string()).and_then(|label| tried::toggle_view(editor, &label)) {
                editor.show_transient_message(why);
            }
            session.touch();
            return;
        }
        _ => return,
    };
    match result {
        Ok(message) => session.note(&message),
        Err(why) => editor.show_transient_message(why),
    }
    session.touch();
}

/// A drop of image files on the window goes to the agent pane, opening it:
/// they are attached to the next message. `None` when the drop holds
/// anything but images, for the other importers.
pub(crate) fn attach_dropped(editor: &mut Editor, paths: &[PathBuf]) -> Option<String> {
    if paths.is_empty() || !paths.iter().all(|path| attachments::is_image_path(path)) {
        return None;
    }
    let session = match session() {
        Ok(session) => session,
        Err(why) => return Some(format!("The agent pane is unavailable: {why}")),
    };
    session.attach(paths.to_vec());
    if !editor.buffer_named_is_showing(PANE_BUFFER) {
        if let Err(error) = editor.runtime_mut().eval_str("(eseq.agent-pane/agent-pane-open)") {
            return Some(format!("The agent pane did not open: {error:?}"));
        }
        editor.refresh_runtime_side_effects();
    }
    Some(match paths.len() {
        1 => "Attaching the image to the next message.".to_string(),
        n => format!("Attaching {n} images to the next message."),
    })
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
/// `backend`, when there is a window, answers the agent's looks at it.
#[cfg_attr(not(target_os = "macos"), allow(unused_mut, unused_variables))]
pub(crate) fn publish(editor: &mut Editor, mut backend: Option<&mut crate::AppBackend>, seen: &mut u64) {
    // Before the pane is first used too: it may open on the tile meant.
    context::remember_focus(editor);
    focus_composer(editor);
    let Some(Ok(session)) = SESSION.get() else { return };
    let requests = session.take_live_requests();
    let served = !requests.is_empty();
    for request in requests.into_iter().filter(|request| !request.abandoned()) {
        if let sequencer::agent::harness::LiveAction::EditDsp { buffer, form, after } = &request.action {
            let (buffer, form, after) = (buffer.clone(), form.clone(), after.clone());
            dsp_edit::start(editor, request, &buffer, &form, after.as_deref());
            continue;
        }
        #[cfg(target_os = "macos")]
        if let Some(result) =
            backend.as_deref_mut().and_then(|backend| live_view::serve(editor, backend, &request.action))
        {
            request.answer(result);
            continue;
        }
        let result = tried::serve(editor, &request.action);
        request.reply(result);
    }
    let answered = dsp_edit::watch();
    if served || answered || tried::forget_closed_views(editor) {
        session.touch();
    }
    let (generation, shape) = session.with_transcript(|transcript| (transcript.generation, transcript.shape));
    if *seen == generation {
        return;
    }
    *seen = generation;
    present_agent_pane(generation, shape);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.refresh_visible_layouts_for_buffer_named(PANE_BUFFER);
    editor.mark_needs_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_keeps_trying_to_focus_the_composer_until_it_is_laid_out() {
        let mut editor = Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        FOCUS_PENDING.with(|pending| pending.set(2));
        focus_composer(&mut editor);
        assert_eq!(FOCUS_PENDING.with(std::cell::Cell::get), 1, "no composer yet: try again next frame");
        focus_composer(&mut editor);
        assert_eq!(FOCUS_PENDING.with(std::cell::Cell::get), 0, "gives up after its frames");
    }

    #[test]
    fn transcript_entries_become_lisp_dicts_the_pane_reads() {
        let transcript = Transcript {
            entries: vec![
                Entry::User { text: "hi".into(), images: vec!["/tmp/knob.png".into()] },
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
            ":images (\"/tmp/knob.png\")",
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
