//! The agent's `edit_dsp`: one DGenLisp form put in place in the user's open
//! code buffer, as one edit they can undo, then compiled as the editor's
//! Eval does. The compile runs on its own thread and reports back frames
//! later, so the tool call is held here ([`watch`]) until the editor says
//! how it went.

use std::cell::RefCell;

use eseqlisp::Editor;
use sequencer::agent::forms::{define_dgen_form, Defined};
use sequencer::agent::harness::LiveRequest;

/// What the editor shows while a preview compiles (`editor_error`).
const COMPILING: &str = "Preview compiling...";
/// Frames to wait for the compile to start before deciding it will not.
const START_FRAMES: u32 = 60;
/// Frames to wait for it to finish (about a minute at 60 fps).
const FINISH_FRAMES: u32 = 3600;

/// An edit made, waiting for its compile.
struct Pending {
    request: LiveRequest,
    /// What the edit did, for the answer.
    done: String,
    frames: u32,
    started: bool,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Make the edit and, when `buffer` is the open editor's code, start its
/// compile and hold `request` until it reports; else answer now.
pub(super) fn start(editor: &mut Editor, request: LiveRequest, buffer: &str, form: &str, after: Option<&str>) {
    match edit(editor, buffer, form, after) {
        Err(why) => request.reply(Err(why)),
        Ok(done) => {
            let shown = crate::presented::presented(|p| p.editor.get().clone());
            let compiles = shown.buffer == buffer && shown.surface == "code";
            if !compiles {
                return request.reply(Ok(format!("{done} Not compiled: {buffer} is not the code the editor is showing.")));
            }
            if let Err(error) = editor.runtime_mut().eval_str("(host-command \"evaluate-editor-source\" (dict))") {
                return request.reply(Ok(format!("{done} The compile did not start: {error:?}")));
            }
            editor.refresh_runtime_side_effects();
            let previous = PENDING.with(|pending| pending.borrow_mut().replace(Pending { request, done, frames: 0, started: false }));
            if let Some(previous) = previous {
                previous.request.reply(Ok(format!("{} A later edit started its own compile.", previous.done)));
            }
        }
    }
}

/// Put the form in place in the buffer as one undoable edit; what it did.
fn edit(editor: &mut Editor, buffer: &str, form: &str, after: Option<&str>) -> Result<String, String> {
    let Some(source) = editor.read_buffer_text(buffer) else {
        return Err(format!("No buffer is named {buffer}"));
    };
    let (next, defined) = define_dgen_form(&source, form, after)?;
    if next == source {
        return Err("That is what the code already says; nothing changed.".to_string());
    }
    if !editor.replace_buffer_text_undoable(buffer, &next) {
        return Err(format!("No buffer is named {buffer}"));
    }
    Ok(match defined {
        Defined::Replaced { line } => format!("Replaced the form at line {line} of {buffer} (unsaved; Cmd+Z undoes it)."),
        Defined::Appended { line } => format!("Added the form at line {line} of {buffer} (unsaved; Cmd+Z undoes it)."),
    })
}

/// Once per frame: answer the held edit once its compile has reported.
pub(super) fn watch() -> bool {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        // A cancelled turn stops waiting; answering it then is harmless.
        let Some(edit) = pending.as_mut() else { return false };
        let error = crate::presented::presented(|p| p.editor.get().error.clone());
        edit.frames += 1;
        let Some(answer) = verdict(&error, &mut edit.started, edit.frames) else { return false };
        let edit = pending.take().expect("checked above");
        edit.request.reply(Ok(format!("{} {answer}", edit.done)));
        true
    })
}

/// How the compile went, from the editor's status this frame, once there
/// is something to say: `started` notes that it was seen compiling.
fn verdict(error: &str, started: &mut bool, frames: u32) -> Option<String> {
    if error == COMPILING {
        *started = true;
        (frames > FINISH_FRAMES).then(|| "It is still compiling; look again in a moment.".to_string())
    } else if *started {
        Some(if error.is_empty() {
            "It compiled and now plays.".to_string()
        } else {
            format!("It did not compile, so the last good version still plays:\n{error}")
        })
    } else {
        (frames > START_FRAMES).then(|| "No compile started; the user can press Eval.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::mode::BufferMode;
    use eseqlisp::{EditorConfig, Runtime};

    #[test]
    fn an_edit_lands_in_the_buffer_as_one_undoable_unsaved_step() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer_with_mode("*code*", "(param gain @default 0.5)\n(def y (* gain 2))\n(out y 1)\n", BufferMode::DGenLisp);
        let done = edit(&mut editor, "*code*", "(def y (* gain 3))", None).unwrap();
        assert_eq!(done, "Replaced the form at line 2 of *code* (unsaved; Cmd+Z undoes it).");
        assert_eq!(editor.read_buffer_text("*code*").unwrap(), "(param gain @default 0.5)\n(def y (* gain 3))\n(out y 1)\n");
        assert!(editor.active_buffer().dirty);
        assert!(edit(&mut editor, "*code*", "(def y (* gain 3))", None).unwrap_err().contains("nothing changed"));
        assert!(edit(&mut editor, "*nope*", "(def y 1)", None).unwrap_err().contains("No buffer"));
    }

    #[test]
    fn the_verdict_waits_for_the_compile_to_start_and_then_to_finish() {
        let mut started = false;
        assert_eq!(verdict("", &mut started, 1), None, "not started yet");
        assert_eq!(verdict(COMPILING, &mut started, 2), None);
        assert!(started);
        assert_eq!(verdict("", &mut started, 3).as_deref(), Some("It compiled and now plays."));
        let mut started = true;
        assert!(verdict("unknown symbol `rnd`", &mut started, 4).unwrap().contains("did not compile"));
        let mut started = false;
        assert!(verdict("", &mut started, START_FRAMES + 1).unwrap().contains("No compile started"));
    }
}
