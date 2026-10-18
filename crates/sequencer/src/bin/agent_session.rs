//! Run agent pane turns without the UI and print the transcript as it
//! streams: the pane's own `AgentSession`, against a built `metal_seq mcp`.
//!
//!   agent_session <metal_seq> <state-dir> <message>…
//!
//! Each further message is sent after the previous turn ends; `/rename
//! <package> <new-name>` and `/reject <package>` act as the pane's card
//! buttons do instead (`_` names the first package the agent wrote). Needs
//! ANTHROPIC_API_KEY; the agent writes Local packages like the pane does
//! (set ESEQ_CONFIG_DIR to keep them out of ~/.eseq.d).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sequencer::agent::harness::{clip, AgentSession, Artifact, Entry, SessionConfig, ToolStatus, Transcript};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(program), Some(state_dir)) = (args.next(), args.next()) else {
        usage();
    };
    let messages: Vec<String> = args.collect();
    if messages.is_empty() {
        usage();
    }
    let state_dir = PathBuf::from(state_dir);
    let mut config = SessionConfig::for_this_app(state_dir).expect("config");
    config.tool_server.program = program.into();
    let session = AgentSession::new(config);
    let started = Instant::now();
    let mut printed = 0;
    let mut seen_generation = None;
    let mut turn_start_usage = (0, 0, 0);
    for message in messages {
        let first = session.with_transcript(|t| t.artifacts.first().map(|a| a.package.clone())).unwrap_or_default();
        let words: Vec<&str> =
            message.split_whitespace().map(|word| if word == "_" { first.as_str() } else { word }).collect();
        let action = match words.as_slice() {
            ["/rename", package, new_name] => Some(session.rename(package, new_name)),
            ["/reject", package] => Some(session.reject(package)),
            _ => None,
        };
        if let Some(result) = action {
            println!("{message}: {}", result.unwrap_or_else(|why| format!("refused: {why}")));
            continue;
        }
        if let Err(error) = session.send(&message) {
            eprintln!("agent_session: {error}");
            std::process::exit(2);
        }
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let generation = session.generation();
            if seen_generation == Some(generation) {
                continue;
            }
            seen_generation = Some(generation);
            let (lines, busy, totals) = session.with_transcript(|t| (final_lines(t, printed), t.busy, tokens(t)));
            printed += lines.len();
            for line in lines {
                println!("[{:>5.0}s] {line}", started.elapsed().as_secs_f64());
            }
            if !busy {
                let (input, output, cache_read) = turn_start_usage;
                println!(
                    "-- turn ended; tokens in {} out {} cache-read {}",
                    totals.0 - input,
                    totals.1 - output,
                    totals.2 - cache_read
                );
                turn_start_usage = totals;
                for artifact in session.with_transcript(|t| t.artifacts.clone()) {
                    println!("{}", render_artifact(&artifact));
                }
                break;
            }
        }
    }
}

fn usage() -> ! {
    eprintln!("usage: agent_session <metal_seq> <state-dir> <message>…");
    std::process::exit(2);
}

fn tokens(t: &Transcript) -> (u64, u64, u64) {
    (t.input_tokens, t.output_tokens, t.cache_read_tokens)
}

/// The entries after `printed` that will not change any more, in order:
/// the last one is still streaming while the turn runs, and a tool stays
/// open until it has run (calls run after their whole reply has streamed).
fn final_lines(transcript: &Transcript, printed: usize) -> Vec<String> {
    let entries = &transcript.entries;
    let open = |index: usize, entry: &Entry| {
        (transcript.busy && index + 1 == entries.len())
            || matches!(entry, Entry::Tool { status: ToolStatus::Running, .. })
    };
    entries
        .iter()
        .enumerate()
        .skip(printed)
        .take_while(|(index, entry)| !open(*index, entry))
        .map(|(_, entry)| render(entry))
        .collect()
}

fn render_artifact(artifact: &Artifact) -> String {
    let mut lines = vec![format!(
        "PACKAGE {} ({}) entries {:?} kinds {:?}",
        artifact.package,
        if artifact.created { "new" } else { "edited" },
        artifact.entries,
        artifact.kinds.iter().map(|decl| format!("{}/{}", decl.module, decl.kind)).collect::<Vec<_>>()
    )];
    for file in &artifact.files {
        lines.push(format!(
            "    {} {} {} lines +{} -{}",
            file.change.as_str(),
            file.name,
            file.lines,
            file.added,
            file.removed
        ));
    }
    lines.join("\n")
}

fn render(entry: &Entry) -> String {
    match entry {
        Entry::User(text) => format!("USER {text}"),
        Entry::Assistant(text) => format!("CLAUDE {text}"),
        Entry::Thinking(text) => format!("THINKING {}", clip(text, 300)),
        Entry::Notice(text) => format!("NOTICE {text}"),
        Entry::Tool { name, input, status, output, images, .. } => format!(
            "TOOL {name} [{}] {}\n    -> {}{}",
            status.as_str(),
            clip(input, 400),
            clip(output, 800).replace('\n', "\n       "),
            if images.is_empty() { String::new() } else { format!("\n    images: {images:?}") }
        ),
    }
}
