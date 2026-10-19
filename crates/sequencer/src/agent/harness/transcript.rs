//! What the agent pane shows: the conversation as entries, updated while a
//! turn streams, with a generation the UI polls to know when to redraw.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::mcp_client::content_text;
use super::workspace::Artifact;

const NOTES_START: &str = "(Since your last turn, in the pane: ";
const NOTES_END: &str = ")\n\n";

/// What the user typed, prefixed with what happened in the pane since the
/// last turn (which the pane showed as notices, not as their words).
pub fn with_notes(notes: &[String], text: &str) -> String {
    if notes.is_empty() {
        text.to_string()
    } else {
        format!("{NOTES_START}{}{NOTES_END}{text}", notes.join(" "))
    }
}

/// `with_notes` undone: the notes, if any, and what the user typed.
fn split_notes(sent: &str) -> (Option<&str>, &str) {
    sent.strip_prefix(NOTES_START)
        .and_then(|rest| rest.split_once(NOTES_END))
        .map_or((None, sent), |(notes, text)| (Some(notes), text))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
}

impl ToolStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User(String),
    Assistant(String),
    /// The model's reasoning, as summarized by the API.
    Thinking(String),
    Tool {
        id: String,
        name: String,
        /// The call's input, compact JSON.
        input: String,
        status: ToolStatus,
        output: String,
        /// PNGs the tool returned, saved for the pane to draw.
        images: Vec<PathBuf>,
    },
    /// Something the user should know that is not the model's words: an
    /// API error, a cancelled turn, a declined request.
    Notice(String),
}

/// `text` cut to `limit` characters, marked when cut: how a transcript is
/// shown.
/// One entry as a line of plain text, for drivers that print the
/// conversation: a tool shows its input, output and images.
pub fn render_entry(entry: &Entry) -> String {
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

pub fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub entries: Vec<Entry>,
    pub busy: bool,
    /// Moves on every change.
    pub generation: u64,
    /// Moves on every change but text streaming into the last entry and
    /// token counts: when the conversation's rows, not their text, change.
    pub shape: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// The packages this conversation wrote, as they stand.
    pub artifacts: Vec<Artifact>,
}

impl Transcript {
    /// The entries a saved conversation's messages (Messages API JSON, as
    /// `conversation.json` holds them) showed while it ran: your text, the
    /// model's thinking, replies and tool calls with their results.
    pub fn from_messages(messages: &[Value]) -> Self {
        let mut transcript = Self::default();
        for message in messages {
            let assistant = message["role"] == "assistant";
            let blocks = match &message["content"] {
                Value::String(text) => vec![json!({ "type": "text", "text": text })],
                Value::Array(blocks) => blocks.clone(),
                _ => Vec::new(),
            };
            for block in &blocks {
                let text = |key: &str| block[key].as_str().unwrap_or_default().to_string();
                match block["type"].as_str() {
                    Some("text") if assistant => transcript.entries.push(Entry::Assistant(text("text"))),
                    Some("text") => {
                        let sent = text("text");
                        let (notes, typed) = split_notes(&sent);
                        transcript.entries.extend(notes.map(|notes| Entry::Notice(notes.to_string())));
                        transcript.entries.push(Entry::User(typed.to_string()));
                    }
                    Some("thinking") => transcript.entries.push(Entry::Thinking(text("thinking"))),
                    Some("tool_use") => transcript.entries.push(Entry::Tool {
                        id: text("id"),
                        name: text("name"),
                        input: block["input"].to_string(),
                        status: ToolStatus::Running,
                        output: String::new(),
                        images: Vec::new(),
                    }),
                    Some("tool_result") => {
                        let output = match &block["content"] {
                            Value::String(text) => text.clone(),
                            Value::Array(parts) => content_text(parts),
                            _ => String::new(),
                        };
                        if let Some((_, status, tool_output, _)) = transcript.tool_mut(&text("tool_use_id")) {
                            *status = if block["is_error"] == true { ToolStatus::Failed } else { ToolStatus::Done };
                            *tool_output = output;
                        }
                    }
                    _ => {}
                }
            }
        }
        // A turn cut off mid-round saved calls without results.
        transcript.fail_running_tools("No result was saved.");
        transcript
    }

    /// The tool row for call `id`: its input, status, output and images.
    fn tool_mut(&mut self, id: &str) -> Option<(&mut String, &mut ToolStatus, &mut String, &mut Vec<PathBuf>)> {
        self.entries.iter_mut().rev().find_map(|entry| match entry {
            Entry::Tool { id: tool_id, input, status, output, images, .. } if tool_id == id => {
                Some((input, status, output, images))
            }
            _ => None,
        })
    }

    fn changed(&mut self) {
        self.generation += 1;
    }

    fn reshaped(&mut self) {
        self.shape += 1;
        self.changed();
    }

    /// The entries from `printed` on that will not change any more, in
    /// order, for a driver printing the conversation as it settles: the last
    /// one is still streaming while the turn runs, and a tool stays open
    /// until it has run.
    pub fn settled_from(&self, printed: usize) -> impl Iterator<Item = &Entry> {
        let last = self.entries.len().saturating_sub(1);
        self.entries
            .iter()
            .enumerate()
            .skip(printed)
            .take_while(move |(index, entry)| {
                !(self.busy && *index == last) && !matches!(entry, Entry::Tool { status: ToolStatus::Running, .. })
            })
            .map(|(_, entry)| entry)
    }

    /// Redraw without a change here: something the pane shows beside the
    /// conversation moved (a package was loaded into the project).
    pub fn touch(&mut self) {
        self.reshaped();
    }

    pub fn set_artifacts(&mut self, artifacts: Vec<Artifact>) {
        if self.artifacts != artifacts {
            self.artifacts = artifacts;
            self.reshaped();
        }
    }

    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.reshaped();
    }

    /// Extend the model's reply, or start it.
    pub fn append_assistant(&mut self, text: &str) {
        match self.entries.last_mut() {
            Some(Entry::Assistant(existing)) => existing.push_str(text),
            _ => return self.push(Entry::Assistant(text.into())),
        }
        self.changed();
    }

    /// Extend the model's reasoning, or start it.
    pub fn append_thinking(&mut self, text: &str) {
        match self.entries.last_mut() {
            Some(Entry::Thinking(existing)) => existing.push_str(text),
            _ => return self.push(Entry::Thinking(text.into())),
        }
        self.changed();
    }

    pub fn add_usage(&mut self, input: u64, output: u64, cache_read: u64) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.cache_read_tokens += cache_read;
        // Once per reply, not per token: the composer's counts wait for the shape.
        self.reshaped();
    }

    pub fn finish_tool(&mut self, id: &str, input: String, status: ToolStatus, output: String, images: Vec<PathBuf>) {
        if let Some((tool_input, tool_status, tool_output, tool_images)) = self.tool_mut(id) {
            *tool_input = input;
            *tool_status = status;
            *tool_output = output;
            *tool_images = images;
            self.reshaped();
        }
    }

    /// Mark the tool rows still running as failed with `why`: the turn
    /// ended before they ran.
    pub fn fail_running_tools(&mut self, why: &str) {
        let mut any = false;
        for entry in &mut self.entries {
            if let Entry::Tool { status: status @ ToolStatus::Running, output, .. } = entry {
                *status = ToolStatus::Failed;
                *output = why.to_string();
                any = true;
            }
        }
        if any {
            self.reshaped();
        }
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        self.reshaped();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_streams_into_the_last_entry_of_its_kind_and_every_change_bumps_the_generation() {
        let mut transcript = Transcript::default();
        transcript.push(Entry::User("hi".into()));
        transcript.append_thinking("Thinking ");
        transcript.append_thinking("more.");
        transcript.append_assistant("Hel");
        transcript.append_assistant("lo");
        assert_eq!(
            transcript.entries,
            vec![Entry::User("hi".into()), Entry::Thinking("Thinking more.".into()), Entry::Assistant("Hello".into())]
        );
        assert_eq!(transcript.generation, 5);
        // New entries reshape it; text streaming into one does not.
        assert_eq!(transcript.shape, 3);
    }

    #[test]
    fn a_saved_conversation_shows_as_the_entries_it_streamed() {
        let messages = [
            json!({ "role": "user", "content": [{ "type": "text", "text": "add 1 and 2" }] }),
            json!({ "role": "assistant", "content": [
                { "type": "thinking", "thinking": "Use eval.", "signature": "s" },
                { "type": "tool_use", "id": "t1", "name": "eval", "input": { "code": "(+ 1 2)" } },
                { "type": "tool_use", "id": "t2", "name": "eval", "input": { "code": "(" } },
            ] }),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": [{ "type": "text", "text": "=> 3" }] },
                { "type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": "unbalanced" },
            ] }),
            json!({ "role": "assistant", "content": [
                { "type": "text", "text": "It is 3." },
                { "type": "tool_use", "id": "t3", "name": "eval", "input": {} },
            ] }),
            json!({ "role": "user", "content": with_notes(&["You loaded markov.".into()], "thanks") }),
        ];
        let tool = |id: &str, input: &str, status, output: &str| Entry::Tool {
            id: id.into(),
            name: "eval".into(),
            input: input.into(),
            status,
            output: output.into(),
            images: Vec::new(),
        };
        assert_eq!(
            Transcript::from_messages(&messages).entries,
            vec![
                Entry::User("add 1 and 2".into()),
                Entry::Thinking("Use eval.".into()),
                tool("t1", r#"{"code":"(+ 1 2)"}"#, ToolStatus::Done, "=> 3"),
                tool("t2", r#"{"code":"("}"#, ToolStatus::Failed, "unbalanced"),
                Entry::Assistant("It is 3.".into()),
                tool("t3", "{}", ToolStatus::Failed, "No result was saved."),
                Entry::Notice("You loaded markov.".into()),
                Entry::User("thanks".into()),
            ]
        );
    }
}
