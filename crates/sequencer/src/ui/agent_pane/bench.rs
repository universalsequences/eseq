//! `metal_seq agent-bench MESSAGE…`: the pane's agent against a headless app
//! standing in for the live one, so its `try` calls change something that
//! can be drawn. Prints the conversation as it settles. Besides messages:
//! `/keep` and `/revert` press the Trying card's buttons, and
//! `/capture BUFFER PATH [KEY]` draws a buffer of the stand-in app to a PNG
//! (clicking the widget keyed KEY first) and
//! `/eval CODE` evaluates there (to mute a track before capturing);
//! `/try CODE` tries forms there as the agent's `try` does.
//! Needs ANTHROPIC_API_KEY; set ESEQ_CONFIG_DIR to keep Keep's init.lisp
//! and the agent's packages out of ~/.eseq.d.

use std::time::{Duration, Instant};

use sequencer::agent::harness::{render_entry, LiveAction};

use super::super::capture::{parse_capture_source, HeadlessSession};
use super::{session, tried};

/// The messages after `agent-bench`, when that is the subcommand.
pub(crate) fn args() -> Option<Vec<String>> {
    let mut args = std::env::args().skip(1);
    (args.next().as_deref() == Some("agent-bench")).then(|| args.collect())
}

pub(crate) fn run(messages: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let script = std::env::temp_dir().join("eseq-agent-bench-setup.lisp");
    let parsed = parse_capture_source(sequencer::authoring_kit::SEQUENCER_CHECK_PROJECT)?;
    let mut app = HeadlessSession::boot(&script, &parsed, None, 0, false)?;
    let agent = session()?;
    let started = Instant::now();
    let mut printed = 0;
    for message in messages {
        let words: Vec<&str> = message.split_whitespace().collect();
        match words.as_slice() {
            ["/keep"] => println!("/keep: {}", tried::keep_in_init(&mut app.editor).unwrap_or_else(|why| why)),
            ["/revert"] => println!("/revert: {}", tried::revert_tried(&mut app.editor).unwrap_or_else(|why| why)),
            ["/capture", buffer, path, click @ ..] => {
                app.settle()?;
                let (png, _) = app.render_png(buffer, None, (1600, 700), click.first().copied(), None)?;
                std::fs::write(path, png)?;
                println!("/capture: {buffer} -> {path}");
            }
            ["/try", ..] => {
                let code = message.trim_start_matches("/try").to_string();
                let result = tried::serve(&mut app.editor, &LiveAction::Try(code));
                app.settle()?;
                println!("/try: {}", result.unwrap_or_else(|why| why));
            }
            ["/eval", ..] => {
                let result = app.editor.runtime_mut().eval_str(message.trim_start_matches("/eval"));
                app.settle()?;
                println!("/eval: {:?}", result.map(|value| value.map(|value| app.editor.runtime().format_value(&value))));
            }
            _ => {
                agent.send(&message)?;
                loop {
                    for request in agent.take_live_requests().into_iter().filter(|request| !request.abandoned()) {
                        let result = tried::serve(&mut app.editor, &request.action);
                        app.settle()?;
                        request.reply(result);
                    }
                    let (lines, busy) =
                        agent.with_transcript(|t| (t.settled_from(printed).map(render_entry).collect::<Vec<_>>(), t.busy));
                    printed += lines.len();
                    for line in lines {
                        println!("[{:>4.0}s] {line}", started.elapsed().as_secs_f64());
                    }
                    if !busy {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    if let Some(dir) = agent.conversation_dir() {
        println!("conversation: {}", dir.join("conversation.json").display());
    }
    Ok(())
}
