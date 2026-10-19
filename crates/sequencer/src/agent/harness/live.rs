//! Tools that act on the user's running app rather than the headless tool
//! server. The turn thread queues a [`LiveRequest`]; the UI thread takes it
//! on its next frame ([`super::AgentSession::take_live_requests`]), runs it
//! and replies.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{lock, ToolResult};

pub const TRY_TOOL: &str = "try";
pub const KEEP_TOOL: &str = "keep_tried";

/// Whether `name` is one of these tools.
pub fn is_live_tool(name: &str) -> bool {
    name == TRY_TOOL || name == KEEP_TOOL
}

/// What a live call asks the app to do.
pub enum LiveAction {
    /// Evaluate these forms as module `user.try`.
    Try(String),
    /// Press the pane's Keep: what is tried goes into init.lisp.
    Keep,
}

/// How long a live call waits for the app before giving up: the UI thread
/// serves requests every frame, so only a stalled app takes this long.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// One live call, waiting for the UI thread.
pub struct LiveRequest {
    pub action: LiveAction,
    reply: mpsc::Sender<Result<String, String>>,
    /// Cleared when the tool call stops waiting (cancelled, timed out).
    waiting: Arc<AtomicBool>,
}

/// Clears a request's `waiting` however its tool call returns.
struct StopWaiting(Arc<AtomicBool>);

impl Drop for StopWaiting {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

impl LiveRequest {
    /// The tool call gave up on this request: the agent was told it failed,
    /// so it must not run.
    pub fn abandoned(&self) -> bool {
        !self.waiting.load(Ordering::Relaxed)
    }

    /// Answer the waiting tool call: what the app now shows, or why not.
    pub fn reply(self, result: Result<String, String>) {
        let _ = self.reply.send(result);
    }
}

/// The live tools' Messages API definitions.
pub fn tools() -> Vec<Value> {
    vec![try_tool(), keep_tool()]
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
    let action = if name == KEEP_TOOL {
        LiveAction::Keep
    } else {
        match input["code"].as_str().filter(|code| !code.trim().is_empty()) {
            Some(code) => LiveAction::Try(code.to_string()),
            None => return ToolResult::error("`code` is required"),
        }
    };
    let (reply, answer) = mpsc::channel();
    let waiting = Arc::new(AtomicBool::new(true));
    let _stop_waiting = StopWaiting(Arc::clone(&waiting));
    lock(queue).push(LiveRequest { action, reply, waiting });
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    loop {
        match answer.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(text)) => return ToolResult::ok(text),
            Ok(Err(why)) => return ToolResult::error(why),
            Err(mpsc::RecvTimeoutError::Disconnected) => return ToolResult::error("The app dropped the request."),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            return ToolResult::error("Stopped.");
        }
        if Instant::now() >= deadline {
            return ToolResult::error("The app did not answer; it may be busy. Try again.");
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
}
