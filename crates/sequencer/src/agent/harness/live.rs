//! Tools that act on the user's running app rather than the headless tool
//! server. The turn thread queues a [`LiveRequest`]; the UI thread takes it
//! on its next frame ([`super::AgentSession::take_live_requests`]), runs it
//! and replies.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{lock, ToolResult};

pub const TRY_TOOL: &str = "try";
pub const KEEP_TOOL: &str = "keep_tried";
pub const LOOK_TOOL: &str = "look";
pub const WHAT_DRAWS_LIVE_TOOL: &str = "what_draws_live";
pub const READ_BUFFER_TOOL: &str = "read_buffer";
pub const VIEW_STATE_LIVE_TOOL: &str = "view_state_live";
pub const DGEN_VALUES_LIVE_TOOL: &str = "dgen_values_live";
pub const EDIT_DSP_TOOL: &str = "edit_dsp";

/// Whether `name` is one of these tools.
pub fn is_live_tool(name: &str) -> bool {
    [TRY_TOOL, KEEP_TOOL, LOOK_TOOL, WHAT_DRAWS_LIVE_TOOL, READ_BUFFER_TOOL, VIEW_STATE_LIVE_TOOL, DGEN_VALUES_LIVE_TOOL, EDIT_DSP_TOOL]
        .contains(&name)
}

/// What a live call asks the app to do.
pub enum LiveAction {
    /// Evaluate these forms as module `user.try`.
    Try(String),
    /// Press the pane's Keep: what is tried goes into init.lisp.
    Keep,
    /// Show the user's window, or the tile showing `buffer`, with its hooks.
    Look { buffer: Option<String> },
    /// The hooks at pixel (x, y) of `look`'s image of `buffer`.
    WhatDraws { buffer: String, x: f32, y: f32 },
    /// The text of the user's open buffer `buffer`, lines `from..=to`
    /// (1-based; None reads from the start or to the end).
    ReadBuffer { buffer: String, from: Option<usize>, to: Option<usize> },
    /// The reactive state the user's drawn view of `buffer` reads.
    ViewState { buffer: String },
    /// What DGenLisp defs `names` in `buffer` come to with the knobs of the
    /// instrument on `track` (1-based; the current track when None).
    /// `inputs` gives host inputs (`pitch`, `gate`) and any other name a value.
    DgenValues { buffer: String, names: Vec<String>, track: Option<usize>, inputs: Vec<(String, f64)> },
    /// Put DGenLisp `form` in place of the form defining its name in the
    /// user's code buffer `buffer` (a new name after `after`, else before the
    /// outputs), then compile it as the editor's Eval does.
    EditDsp { buffer: String, form: String, after: Option<String> },
}

/// How long a live call waits for the app before giving up: the UI thread
/// serves requests every frame, so only a stalled app takes this long.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a live call waits for a request the app has already started:
/// the action is under way, so giving up at [`ANSWER_TIMEOUT`] would tell
/// the agent it failed while it still takes effect.
const RUNNING_TIMEOUT: Duration = Duration::from_secs(300);

/// Queued; the UI thread has not taken it yet.
const PENDING: u8 = 0;
/// The UI thread took it and will reply: the tool call waits for that.
const RUNNING: u8 = 1;
/// The tool call gave up before the UI thread took it: it must not run.
const ABANDONED: u8 = 2;

/// One live call, waiting for the UI thread.
pub struct LiveRequest {
    pub action: LiveAction,
    reply: mpsc::Sender<ToolResult>,
    /// [`PENDING`], [`RUNNING`] or [`ABANDONED`]; whichever side moves it off
    /// `PENDING` first decides whether the request runs.
    state: Arc<AtomicU8>,
}

/// Gives up on `state` unless the UI thread has taken it.
fn give_up(state: &AtomicU8) -> bool {
    state.compare_exchange(PENDING, ABANDONED, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// Gives up on a request however its tool call returns, unless it is running.
struct StopWaiting(Arc<AtomicU8>);

impl Drop for StopWaiting {
    fn drop(&mut self) {
        give_up(&self.0);
    }
}

impl LiveRequest {
    /// Whether the tool call gave up on this request: the agent was told it
    /// failed, so it must not run. When it has not, this claims the request
    /// for the caller, which must then run it and reply: from here on the
    /// tool call waits for that reply rather than timing out. Call it right
    /// before running the request.
    pub fn abandoned(&self) -> bool {
        match self.state.compare_exchange(PENDING, RUNNING, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) | Err(RUNNING) => false,
            Err(_) => true,
        }
    }

    /// Answer the waiting tool call: what the app now shows, or why not.
    pub fn reply(self, result: Result<String, String>) {
        self.answer(match result {
            Ok(text) => ToolResult::ok(text),
            Err(why) => ToolResult::error(why),
        });
    }

    /// Answer the waiting tool call with a whole result (an image too).
    pub fn answer(self, result: ToolResult) {
        let _ = self.reply.send(result);
    }
}

/// The live tools' Messages API definitions.
pub fn tools() -> Vec<Value> {
    vec![try_tool(), keep_tool(), look_tool(), what_draws_live_tool(), read_buffer_tool(), view_state_live_tool(), dgen_values_live_tool(), edit_dsp_tool()]
}

fn edit_dsp_tool() -> Value {
    json!({
        "name": EDIT_DSP_TOOL,
        "description": "Change an instrument's or effect's DGenLisp in the user's open code editor (its \
            Code surface): `form` is one `def`, `param` or `defmacro`, which takes the place of the \
            form defining the same name; a new name goes after the form defining `after`, or else \
            just before the outputs. The edit lands in their buffer as one step they can undo \
            (Cmd+Z), unsaved, and is compiled and swapped in as the editor's Eval does; the result \
            says whether it compiled, with the errors if not. Saving stays theirs. Use it only when \
            they ask for a change; read the code first (read_buffer) and keep their names and \
            comments.",
        "input_schema": {
            "type": "object",
            "properties": {
                "buffer": {"type": "string", "description": "The code buffer, e.g. `*instrument-code:user:modalys-pluck*`"},
                "form": {"type": "string", "description": "One top-level def, param or defmacro (the comments above the old form stay)"},
                "after": {"type": "string", "description": "For a new name: the def it should follow"}
            },
            "required": ["buffer", "form"]
        }
    })
}

fn dgen_values_live_tool() -> Value {
    json!({
        "name": DGEN_VALUES_LIVE_TOOL,
        "description": "Work out what DGenLisp defs come to with the knobs as the user has them now: \
            reads the instrument or effect code open in their window (unsaved edits included) and \
            the current values of the instrument's params on a track, then evaluates the named \
            top-level defs. Arithmetic, comparisons, gswitch, iota and math are evaluated \
            (vectors element-wise); a def that reads a signal (in, histories, oscillators, macros) \
            is reported as varying per sample. Use it to explain code with real numbers (\"at \
            brass B is 4.1e-4, so mode 80 sits 1.6% sharp\"). It changes nothing.",
        "input_schema": {
            "type": "object",
            "properties": {
                "buffer": {"type": "string", "description": "The code buffer, e.g. `*instrument-code:user:modalys-pluck*`"},
                "names": {"type": "array", "items": {"type": "string"}, "description": "Top-level defs or params to work out"},
                "track": {"type": "integer", "description": "The track (from 1) whose instrument's knobs to use; the current track when left out"},
                "inputs": {"type": "object", "description": "Values for host inputs and other names, e.g. {\"pitch\": 261.63, \"gate\": 1}: a def reading an input varies until it has one"}
            },
            "required": ["buffer", "names"]
        }
    })
}

fn view_state_live_tool() -> Value {
    json!({
        "name": VIEW_STATE_LIVE_TOOL,
        "description": "The state a view in the user's window is showing now: every reactive field its \
            drawn widgets read (defstates, kind fields, host-published values like SEQ.*), with its \
            value. Use it to learn what the user sees (which track is selected, which editor is \
            open) without a screenshot; follow a field's module with form_source to see the code \
            behind it. Fields that differ only by a number fold into one row. A hidden tab is not \
            drawn and reads nothing. `view_state` does the same on the headless copy. It changes \
            nothing.",
        "input_schema": {
            "type": "object",
            "properties": {
                "buffer": {"type": "string", "description": "A buffer (or tab) the <app-context> block names, e.g. `*sequencer*`"}
            },
            "required": ["buffer"]
        }
    })
}

fn read_buffer_tool() -> Value {
    json!({
        "name": READ_BUFFER_TOOL,
        "description": "Read the text of a buffer open in the user's window, as it is on their screen: \
            unsaved edits included, which files on disk and the headless copy do not have. Lines \
            come numbered as the user's status line counts them. Use it for code the user asks \
            about (an instrument's dsp.lisp, a module they have open); the <app-context> block \
            names the buffers. It changes nothing.",
        "input_schema": {
            "type": "object",
            "properties": {
                "buffer": {"type": "string", "description": "The buffer's name, e.g. `*instrument-code:user:modalys-pluck*`"},
                "from": {"type": "integer", "description": "First line to read (1-based); the start when left out"},
                "to": {"type": "integer", "description": "Last line to read; the end (up to a limit) when left out"}
            },
            "required": ["buffer"]
        }
    })
}

fn look_tool() -> Value {
    json!({
        "name": LOOK_TOOL,
        "description": "See the user's window as they see it now: their project, selection, open \
            tiles and installed packages (the headless copy `capture` draws has its own). Without \
            `buffer`: an image of the whole window and the buffers its tiles show. With `buffer`: \
            an image of that tile and the hooks that restyle what is in it (parts, theme slots, \
            building functions). Look first when the user refers to what is on their screen; it \
            changes nothing.",
        "input_schema": {
            "type": "object",
            "properties": {
                "buffer": {"type": "string", "description": "A buffer one of the user's tiles shows, e.g. `*fx*`"}
            }
        }
    })
}

fn what_draws_live_tool() -> Value {
    json!({
        "name": WHAT_DRAWS_LIVE_TOOL,
        "description": "What draws pixel (x, y) of `look`'s image of a tile of the user's window: the \
            widgets there, topmost first, each with the hooks that restyle it, least fragile \
            first: theme slots (override-theme-slot), its part (restyle), the function that built \
            it (override :around). Look at the tile first; it changes nothing.",
        "input_schema": {
            "type": "object",
            "properties": {
                "buffer": {"type": "string", "description": "The tile's buffer, as given to `look`"},
                "x": {"type": "integer", "description": "Pixel column in look's image"},
                "y": {"type": "integer", "description": "Pixel row in look's image"}
            },
            "required": ["buffer", "x", "y"]
        }
    })
}

fn keep_tool() -> Value {
    json!({
        "name": KEEP_TOOL,
        "description": "Save what you are trying (`try`) into the user's init.lisp, creating it if need \
            be, as the pane's Keep button does: use it when the user tells you to keep the change. \
            Returns what was kept, or why not.",
        "input_schema": {"type": "object", "properties": {}}
    })
}

fn try_tool() -> Value {
    json!({
        "name": TRY_TOOL,
        "description": "Evaluate forms in the user's running eseq, where they see the result at once: \
            the way to show a change to how the app looks or behaves (an `override` of a factory \
            function, a theme tweak) before anything is saved. The forms run as module `user.try`, \
            together with what earlier `try` calls left: a definition or override replaces the one \
            of the same name, other forms are added. Include the `(import …)` lines your forms \
            need. The pane then offers the user Keep, which adds the forms to their init.lisp (you \
            can do the same with `keep_tried` when they ask), and Revert, which removes user.try's \
            overrides. A failed try changes nothing and returns \
            the errors. Use `capture` on the headless app to check layouts first when unsure; this \
            tool returns no image.",
        "input_schema": {
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "One or more top-level forms"}
            },
            "required": ["code"]
        }
    })
}

/// Queue live tool `name` for the UI thread and wait for its answer, giving
/// up when the turn is cancelled or the app does not answer in time.
pub(super) fn call(queue: &Mutex<Vec<LiveRequest>>, cancel: &AtomicBool, name: &str, input: &Value) -> ToolResult {
    let text = |key: &str| input[key].as_str().filter(|value| !value.trim().is_empty()).map(str::to_string);
    let action = match name {
        KEEP_TOOL => LiveAction::Keep,
        LOOK_TOOL => LiveAction::Look { buffer: text("buffer") },
        WHAT_DRAWS_LIVE_TOOL => match (text("buffer"), input["x"].as_f64(), input["y"].as_f64()) {
            (Some(buffer), Some(x), Some(y)) => LiveAction::WhatDraws { buffer, x: x as f32, y: y as f32 },
            _ => return ToolResult::error("`buffer`, `x` and `y` are required"),
        },
        READ_BUFFER_TOOL => match text("buffer") {
            Some(buffer) => {
                let line = |key: &str| input[key].as_u64().map(|line| line as usize);
                LiveAction::ReadBuffer { buffer, from: line("from"), to: line("to") }
            }
            None => return ToolResult::error("`buffer` is required"),
        },
        EDIT_DSP_TOOL => match (text("buffer"), text("form")) {
            (Some(buffer), Some(form)) => LiveAction::EditDsp { buffer, form, after: text("after") },
            _ => return ToolResult::error("`buffer` and `form` are required"),
        },
        DGEN_VALUES_LIVE_TOOL => match text("buffer") {
            Some(buffer) => {
                let names = input["names"]
                    .as_array()
                    .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                let track = input["track"].as_u64().map(|track| track as usize);
                let inputs = input["inputs"]
                    .as_object()
                    .map(|inputs| inputs.iter().filter_map(|(name, value)| Some((name.clone(), value.as_f64()?))).collect())
                    .unwrap_or_default();
                LiveAction::DgenValues { buffer, names, track, inputs }
            }
            None => return ToolResult::error("`buffer` is required"),
        },
        VIEW_STATE_LIVE_TOOL => match text("buffer") {
            Some(buffer) => LiveAction::ViewState { buffer },
            None => return ToolResult::error("`buffer` is required"),
        },
        TRY_TOOL => match text("code") {
            Some(code) => LiveAction::Try(code),
            None => return ToolResult::error("`code` is required"),
        },
        _ => return ToolResult::error(format!("`{name}` is not a live tool")),
    };
    let (reply, answer) = mpsc::channel();
    let state = Arc::new(AtomicU8::new(PENDING));
    let _stop_waiting = StopWaiting(Arc::clone(&state));
    lock(queue).push(LiveRequest { action, reply, state: Arc::clone(&state) });
    wait(&answer, &state, cancel, ANSWER_TIMEOUT, RUNNING_TIMEOUT)
}

/// Wait for the UI thread's answer. Cancelling or `timeout` gives up only
/// while the request is still queued; once the app has started it, the call
/// waits (up to `running_timeout` from the start) for the reply, so the agent
/// is never told a call failed that then takes effect.
fn wait(
    answer: &mpsc::Receiver<ToolResult>,
    state: &AtomicU8,
    cancel: &AtomicBool,
    timeout: Duration,
    running_timeout: Duration,
) -> ToolResult {
    let started = Instant::now();
    loop {
        match answer.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => return ToolResult::error("The app dropped the request."),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let waited = started.elapsed();
        if cancel.load(Ordering::Relaxed) && give_up(state) {
            return ToolResult::error("Stopped.");
        }
        if waited >= timeout && give_up(state) {
            return ToolResult::error("The app did not answer; it may be busy. Try again.");
        }
        if waited >= running_timeout {
            return ToolResult::error(
                "The app started this call but has not finished it; it may still take effect. Check before retrying.",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_try_call_waits_for_the_app_to_answer() {
        let queue = std::sync::Arc::new(Mutex::new(Vec::<LiveRequest>::new()));
        let app = {
            let queue = std::sync::Arc::clone(&queue);
            std::thread::spawn(move || loop {
                if let Some(request) = lock(&queue).pop() {
                    let LiveAction::Try(code) = &request.action else { panic!("a try") };
                    let code = code.clone();
                    request.reply(Ok(format!("showing {code}")));
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            })
        };
        let result = call(&queue, &AtomicBool::new(false), TRY_TOOL, &json!({"code": "(def x 1)"}));
        app.join().unwrap();
        assert!(!result.is_error);
        assert_eq!(result.text(), "showing (def x 1)");
    }

    #[test]
    fn a_cancelled_turn_stops_waiting() {
        let queue = Mutex::new(Vec::new());
        let result = call(&queue, &AtomicBool::new(true), TRY_TOOL, &json!({"code": "(def x 1)"}));
        assert!(result.is_error);
        assert_eq!(result.text(), "Stopped.");
        let left = lock(&queue).pop().expect("the request stays queued");
        assert!(left.abandoned(), "the app must skip a request nobody waits for");
        assert!(call(&queue, &AtomicBool::new(false), TRY_TOOL, &json!({})).is_error, "no code is an error");
    }

    #[test]
    fn a_started_request_is_answered_past_the_timeout_and_cancel() {
        let (reply, answer) = mpsc::channel();
        let state = Arc::new(AtomicU8::new(PENDING));
        let request = LiveRequest { action: LiveAction::Keep, reply, state: Arc::clone(&state) };
        assert!(!request.abandoned(), "a pending request is claimed");
        assert!(!request.abandoned(), "claiming is idempotent");
        let app = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            request.reply(Ok("kept".into()));
        });
        let result = wait(&answer, &state, &AtomicBool::new(true), Duration::ZERO, Duration::from_secs(30));
        app.join().unwrap();
        assert!(!result.is_error, "a running request must not be reported as failed");
        assert_eq!(result.text(), "kept");
    }

    #[test]
    fn a_timed_out_request_is_skipped_by_the_app() {
        let (reply, answer) = mpsc::channel();
        let state = Arc::new(AtomicU8::new(PENDING));
        let request = LiveRequest { action: LiveAction::Keep, reply, state: Arc::clone(&state) };
        let result = wait(&answer, &state, &AtomicBool::new(false), Duration::ZERO, Duration::from_secs(30));
        assert_eq!(result.text(), "The app did not answer; it may be busy. Try again.");
        assert!(request.abandoned());
    }
}
