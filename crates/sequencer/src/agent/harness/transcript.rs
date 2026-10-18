//! What the agent pane shows: the conversation as entries, updated while a
//! turn streams, with a generation the UI polls to know when to redraw.

use std::path::PathBuf;

use super::workspace::Artifact;

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
    pub generation: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// The packages this conversation wrote, as they stand.
    pub artifacts: Vec<Artifact>,
}

impl Transcript {
    fn changed(&mut self) {
        self.generation += 1;
    }

    /// Redraw without a change here: something the pane shows beside the
    /// conversation moved (a package was loaded into the project).
    pub fn touch(&mut self) {
        self.changed();
    }

    pub fn set_artifacts(&mut self, artifacts: Vec<Artifact>) {
        if self.artifacts != artifacts {
            self.artifacts = artifacts;
            self.changed();
        }
    }

    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.changed();
    }

    /// Extend the model's reply, or start it.
    pub fn append_assistant(&mut self, text: &str) {
        match self.entries.last_mut() {
            Some(Entry::Assistant(existing)) => existing.push_str(text),
            _ => self.entries.push(Entry::Assistant(text.into())),
        }
        self.changed();
    }

    /// Extend the model's reasoning, or start it.
    pub fn append_thinking(&mut self, text: &str) {
        match self.entries.last_mut() {
            Some(Entry::Thinking(existing)) => existing.push_str(text),
            _ => self.entries.push(Entry::Thinking(text.into())),
        }
        self.changed();
    }

    pub fn add_usage(&mut self, input: u64, output: u64, cache_read: u64) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.cache_read_tokens += cache_read;
        self.changed();
    }

    pub fn finish_tool(&mut self, id: &str, input: String, status: ToolStatus, output: String, images: Vec<PathBuf>) {
        let found = self.entries.iter_mut().rev().find_map(|entry| match entry {
            Entry::Tool { id: tool_id, input, status, output, images, .. } if tool_id == id => {
                Some((input, status, output, images))
            }
            _ => None,
        });
        if let Some((tool_input, tool_status, tool_output, tool_images)) = found {
            *tool_input = input;
            *tool_status = status;
            *tool_output = output;
            *tool_images = images;
            self.changed();
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
            self.changed();
        }
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        self.changed();
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
    }
}
