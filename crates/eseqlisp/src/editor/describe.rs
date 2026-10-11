//! `describe-function`: the `*Help*` view of a symbol's signature and docs,
//! from a Lisp definition header (docs/command-palette-spec.md §3.3) or a
//! documented native.

use super::{Editor, MinibufferMode};
use crate::mode::BufferMode;

pub(super) const HELP_BUFFER: &str = "*Help*";

impl Editor {
    pub(super) fn start_describe_function(&mut self) {
        self.completion = None;
        self.minibuffer = None;
        let candidates = self.collect_mx_candidates();
        self.minibuffer_input =
            Some(MinibufferMode::DescribeFunction { input: String::new(), candidates, selected: 0 });
    }

    /// Shows `name`'s help in the `*Help*` buffer, replacing what it held.
    pub fn describe_function(&mut self, name: &str) {
        let text = self.describe_function_text(name);
        let id = self.upsert_read_only_scratch_buffer_with_mode(HELP_BUFFER, &text, BufferMode::ESeqLisp);
        if let Some(buffer) = self.buffers.iter_mut().find(|buffer| buffer.id == id) {
            buffer.cursor = (0, 0);
            buffer.scroll_top = 0;
        }
        self.set_active_buffer(id);
    }

    pub fn describe_function_text(&mut self, name: &str) -> String {
        if let Some(header) = self.runtime.definition_header(name) {
            let mut text = format!("{}\n\n", header.signature);
            text.push_str(header.doc.as_deref().unwrap_or("Not documented."));
            text.push('\n');
            if let Some(title) = &header.command {
                text.push_str(&format!("\nCommand: {title}\n"));
                if header.choices.is_some() {
                    let prompt = header.choice_prompt().unwrap_or_else(|| "Value".to_string());
                    text.push_str(&format!("Choice: {prompt}\n"));
                }
            }
            if let Some(group) = &header.group {
                text.push_str(&format!("Group: {group}\n"));
            }
            return text;
        }
        match self.runtime.completion_metadata_for(name) {
            Some(metadata) => format!("{}\n\n{}\n", metadata.signature, metadata.docs),
            None => format!("{name}\n\nNot documented.\n"),
        }
    }
}
