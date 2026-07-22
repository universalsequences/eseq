use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use eseqlisp::{BufferMode, Editor};
use sequencer::sample_db::SampleDb;
use sequencer::sample_import::{
    import_staged_samples, normalize_tags, stage_paths, ImportSummary, StagedSample,
    StagedSampleStatus,
};

use eseqlisp::editor::ViewMode;

const BUFFER_NAME: &str = "*sample-import*";
const MODE_NAME: &str = "sample-import-mode";
const DEFAULT_WRAP_WIDTH: usize = 96;
const MIN_WRAP_WIDTH: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImportEdit {
    Title { input: String },
    Tags { input: String },
    BatchTags { input: String },
}

#[derive(Debug, Clone)]
pub(crate) struct SampleImportSession {
    staged: Vec<StagedSample>,
    selected: usize,
    batch_tags: Vec<String>,
    tag_candidates: Vec<String>,
    edit: Option<ImportEdit>,
    pending_control_c: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportKeyOutcome {
    Handled,
    Commit,
    Cancel,
    Ignored,
}

impl SampleImportSession {
    pub(crate) fn from_drop(
        paths: Vec<PathBuf>,
        db_path: &std::path::Path,
    ) -> Result<Self, String> {
        let db = SampleDb::open(db_path)
            .map_err(|error| format!("failed to open {}: {error}", db_path.display()))?;
        let staged = stage_paths(&paths, &db);
        let tag_candidates = db
            .list_tags()
            .map_err(|error| format!("failed to list sample tags: {error}"))?;
        Ok(Self {
            staged,
            selected: 0,
            batch_tags: Vec::new(),
            tag_candidates,
            edit: None,
            pending_control_c: false,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> ImportKeyOutcome {
        if key.kind != KeyEventKind::Press {
            return ImportKeyOutcome::Handled;
        }
        if self.edit.is_some() {
            return self.handle_edit_key(key);
        }
        if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.pending_control_c {
                self.pending_control_c = false;
                return ImportKeyOutcome::Commit;
            }
            self.pending_control_c = true;
            return ImportKeyOutcome::Handled;
        }
        self.pending_control_c = false;
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) | (KeyCode::Esc, KeyModifiers::NONE) => {
                ImportKeyOutcome::Cancel
            }
            (KeyCode::Enter, KeyModifiers::NONE)
            | (KeyCode::Char('n'), KeyModifiers::NONE)
            | (KeyCode::Down, KeyModifiers::NONE) => {
                self.move_selection(1);
                ImportKeyOutcome::Handled
            }
            (KeyCode::Char('p'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                self.move_selection(-1);
                ImportKeyOutcome::Handled
            }
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                let input = self
                    .selected_sample()
                    .map(|sample| sample.tags.join(", "))
                    .unwrap_or_default();
                self.edit = Some(ImportEdit::Tags { input });
                ImportKeyOutcome::Handled
            }
            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                let input = self
                    .selected_sample()
                    .map(|sample| sample.title.clone())
                    .unwrap_or_default();
                self.edit = Some(ImportEdit::Title { input });
                ImportKeyOutcome::Handled
            }
            (KeyCode::Char('b'), KeyModifiers::NONE) => {
                self.edit = Some(ImportEdit::BatchTags {
                    input: self.batch_tags.join(", "),
                });
                ImportKeyOutcome::Handled
            }
            _ => ImportKeyOutcome::Ignored,
        }
    }

    pub(crate) fn commit(
        &self,
        db_path: &std::path::Path,
        sample_dir: &std::path::Path,
    ) -> Result<ImportSummary, String> {
        let mut db = SampleDb::open(db_path)
            .map_err(|error| format!("failed to open {}: {error}", db_path.display()))?;
        Ok(import_staged_samples(
            &self.staged,
            &self.batch_tags,
            sample_dir,
            &mut db,
        ))
    }

    pub(crate) fn render_into_editor(&self, editor: &mut Editor) {
        ensure_import_buffer(editor);
        let wrap_width = import_buffer_wrap_width(editor);
        let rendered = self.render_text(wrap_width);
        if let Some(buffer) = editor
            .buffers
            .iter_mut()
            .find(|buffer| buffer.name == BUFFER_NAME)
        {
            buffer.set_text(&rendered.text);
            buffer.read_only = true;
            buffer.mode = BufferMode::Named(MODE_NAME.to_string());
            buffer.view_mode = ViewMode::TextOnly;
            if rendered.selected_line < buffer.lines.len() {
                buffer.cursor = (rendered.selected_line, 0);
            }
        }
        editor.mark_needs_redraw();
    }

    fn render_text(&self, wrap_width: usize) -> RenderedImportText {
        let ready = self
            .staged
            .iter()
            .filter(|sample| matches!(sample.status, StagedSampleStatus::Ready))
            .count();
        let duplicates = self
            .staged
            .iter()
            .filter(|sample| matches!(sample.status, StagedSampleStatus::Duplicate))
            .count();
        let failed = self
            .staged
            .iter()
            .filter(|sample| matches!(sample.status, StagedSampleStatus::Error(_)))
            .count();
        let mut lines = Vec::new();
        push_wrapped(&mut lines, "Sample Import", "", wrap_width);
        push_wrapped(
            &mut lines,
            &format!(
                "ready: {ready}  duplicates: {duplicates}  failed: {failed}  batch tags: {}",
                display_tags(&self.batch_tags)
            ),
            "  ",
            wrap_width,
        );
        push_wrapped(
            &mut lines,
            "keys: n/RET next, p previous, e tags, t title, b batch tags, TAB complete while editing, C-c C-c import, q cancel",
            "      ",
            wrap_width,
        );
        lines.push(String::new());
        match &self.edit {
            Some(ImportEdit::Title { input }) => {
                push_wrapped(
                    &mut lines,
                    &format!("EDIT title: {input}"),
                    "  ",
                    wrap_width,
                );
                lines.push(String::new());
            }
            Some(ImportEdit::Tags { input }) => {
                push_wrapped(
                    &mut lines,
                    &format!("EDIT sample tags: {input}"),
                    "  ",
                    wrap_width,
                );
                lines.push(String::new());
            }
            Some(ImportEdit::BatchTags { input }) => {
                push_wrapped(
                    &mut lines,
                    &format!("EDIT batch tags: {input}"),
                    "  ",
                    wrap_width,
                );
                lines.push(String::new());
            }
            None => {
                lines.push("samples".to_string());
                lines.push(String::new());
            }
        }
        if self.staged.is_empty() {
            lines.push("No supported audio files found.".to_string());
            return RenderedImportText {
                text: lines.join("\n"),
                selected_line: 0,
            };
        }
        let mut selected_line = lines.len();
        for (index, sample) in self.staged.iter().enumerate() {
            if index == self.selected {
                selected_line = lines.len();
            }
            let marker = if index == self.selected { ">" } else { " " };
            let status = match &sample.status {
                StagedSampleStatus::Ready => "ready".to_string(),
                StagedSampleStatus::Duplicate => "duplicate".to_string(),
                StagedSampleStatus::Error(error) => format!("error: {error}"),
            };
            push_wrapped(
                &mut lines,
                &format!("{marker} {:03} {status}", index + 1),
                "      ",
                wrap_width,
            );
            push_wrapped(
                &mut lines,
                &format!("    title: {}", sample.title),
                "           ",
                wrap_width,
            );
            push_wrapped(
                &mut lines,
                &format!("    tags: {}", display_tags(&sample.tags)),
                "          ",
                wrap_width,
            );
            push_wrapped(
                &mut lines,
                &format!("    path: {}", sample.source_path.display()),
                "          ",
                wrap_width,
            );
            lines.push(String::new());
        }
        RenderedImportText {
            text: lines.join("\n"),
            selected_line,
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) -> ImportKeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.edit = None;
                ImportKeyOutcome::Handled
            }
            KeyCode::Enter => {
                self.apply_edit();
                ImportKeyOutcome::Handled
            }
            KeyCode::Backspace => {
                if let Some(input) = self.edit_input_mut() {
                    input.pop();
                }
                ImportKeyOutcome::Handled
            }
            KeyCode::Tab => {
                self.complete_edit_tag();
                ImportKeyOutcome::Handled
            }
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                if let Some(input) = self.edit_input_mut() {
                    input.push(c);
                }
                ImportKeyOutcome::Handled
            }
            _ => ImportKeyOutcome::Handled,
        }
    }

    fn apply_edit(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        match edit {
            ImportEdit::Title { input } => {
                if let Some(sample) = self.selected_sample_mut() {
                    let title = input.trim();
                    if !title.is_empty() {
                        sample.title = title.to_string();
                    }
                }
            }
            ImportEdit::Tags { input } => {
                let tags = parse_tags(&input);
                self.add_tag_candidates(&tags);
                if let Some(sample) = self.selected_sample_mut() {
                    sample.tags = tags;
                }
            }
            ImportEdit::BatchTags { input } => {
                let tags = parse_tags(&input);
                self.add_tag_candidates(&tags);
                self.batch_tags = tags;
            }
        }
    }

    fn complete_edit_tag(&mut self) {
        let Some((prefix_start, prefix_lower)) = self.edit_input_mut().and_then(|input| {
            let (prefix_start, prefix) = tag_prefix(input);
            (!prefix.is_empty()).then(|| (prefix_start, prefix.to_lowercase()))
        }) else {
            return;
        };
        let Some(candidate) = self
            .tag_candidates
            .iter()
            .find(|tag| tag.to_lowercase().starts_with(&prefix_lower))
            .cloned()
        else {
            return;
        };
        if let Some(input) = self.edit_input_mut() {
            input.truncate(prefix_start);
            input.push_str(&candidate);
        }
    }

    fn edit_input_mut(&mut self) -> Option<&mut String> {
        match &mut self.edit {
            Some(ImportEdit::Title { input })
            | Some(ImportEdit::Tags { input })
            | Some(ImportEdit::BatchTags { input }) => Some(input),
            None => None,
        }
    }

    fn selected_sample(&self) -> Option<&StagedSample> {
        self.staged.get(self.selected)
    }

    fn selected_sample_mut(&mut self) -> Option<&mut StagedSample> {
        self.staged.get_mut(self.selected)
    }

    fn move_selection(&mut self, delta: isize) {
        if self.staged.is_empty() {
            self.selected = 0;
            return;
        }
        let next =
            (self.selected as isize + delta).clamp(0, self.staged.len().saturating_sub(1) as isize);
        self.selected = next as usize;
    }

    fn add_tag_candidates(&mut self, tags: &[String]) {
        for tag in tags {
            if !self
                .tag_candidates
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(tag))
            {
                self.tag_candidates.push(tag.clone());
            }
        }
        self.tag_candidates.sort_by_key(|tag| tag.to_lowercase());
    }
}

struct RenderedImportText {
    text: String,
    selected_line: usize,
}

pub(crate) fn ensure_import_buffer(editor: &mut Editor) {
    let sequencer_tile_id = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*sequencer*")
        .and_then(|buffer_idx| {
            editor
                .tile_root
                .find_leaf_by_buffer_idx(buffer_idx)
                .map(|leaf| leaf.id)
        });
    let id = if let Some(id) = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == BUFFER_NAME)
        .map(|buffer| buffer.id)
    {
        id
    } else {
        editor.create_scratch_buffer(BUFFER_NAME, "", BufferMode::Named(MODE_NAME.to_string()))
    };
    if !editor.swap_buffer_in_tile_showing("*sequencer*", BUFFER_NAME) {
        editor.set_active_buffer(id);
    } else if let Some(tile_id) = sequencer_tile_id {
        editor.switch_active_tile(tile_id);
    }
    let Some(buffer) = editor.buffers.iter_mut().find(|buffer| buffer.id == id) else {
        return;
    };
    buffer.read_only = true;
    buffer.view_mode = ViewMode::TextOnly;
    buffer.mode = BufferMode::Named(MODE_NAME.to_string());
}

pub(crate) fn switch_to_sequencer(editor: &mut Editor) {
    if let Some(id) = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*sequencer*")
        .map(|buffer| buffer.id)
    {
        editor.set_active_buffer(id);
    }
}

fn parse_tags(input: &str) -> Vec<String> {
    normalize_tags(
        &input
            .split(',')
            .map(|tag| tag.trim().to_string())
            .collect::<Vec<_>>(),
    )
}

fn display_tags(tags: &[String]) -> String {
    if tags.is_empty() {
        "-".to_string()
    } else {
        tags.join(", ")
    }
}

fn import_buffer_wrap_width(editor: &Editor) -> usize {
    editor
        .tile_body_rect(editor.active_tile)
        .map(|rect| rect.width.floor().max(0.0) as usize)
        .unwrap_or(DEFAULT_WRAP_WIDTH)
        .saturating_sub(4)
        .max(MIN_WRAP_WIDTH)
}

fn push_wrapped(lines: &mut Vec<String>, text: &str, continuation_indent: &str, width: usize) {
    let mut wrapped = wrap_text(text, continuation_indent, width);
    if wrapped.is_empty() {
        lines.push(String::new());
    } else {
        lines.append(&mut wrapped);
    }
}

fn wrap_text(text: &str, continuation_indent: &str, width: usize) -> Vec<String> {
    let width = width.max(MIN_WRAP_WIDTH);
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let separator = if current.is_empty() { 0 } else { 1 };
        if current.chars().count() + separator + word.chars().count() <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            out.push(current);
            current = continuation_indent.to_string();
        }
        append_wrapped_word(&mut out, &mut current, word, continuation_indent, width);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn append_wrapped_word(
    out: &mut Vec<String>,
    current: &mut String,
    word: &str,
    continuation_indent: &str,
    width: usize,
) {
    for ch in word.chars() {
        if current.chars().count() >= width {
            out.push(std::mem::take(current));
            current.push_str(continuation_indent);
        }
        current.push(ch);
    }
}

fn tag_prefix(input: &str) -> (usize, &str) {
    let start = input.rfind(',').map(|index| index + 1).unwrap_or(0);
    let trimmed_start = input[start..]
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(offset, _)| start + offset)
        .unwrap_or(input.len());
    (trimmed_start, &input[trimmed_start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_sample(path: &str) -> StagedSample {
        StagedSample {
            source_path: PathBuf::from(path),
            hash: Some("abc".to_string()),
            title: "Kick".to_string(),
            tags: Vec::new(),
            status: StagedSampleStatus::Ready,
        }
    }

    #[test]
    fn edit_sample_tags_uses_completion_candidates() {
        let mut session = SampleImportSession {
            staged: vec![ready_sample("kick.wav")],
            selected: 0,
            batch_tags: Vec::new(),
            tag_candidates: vec!["drum".to_string(), "snare".to_string()],
            edit: None,
            pending_control_c: false,
        };
        assert_eq!(
            session.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
            ImportKeyOutcome::Handled
        );
        for ch in ['d', 'r'] {
            session.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        session.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        session.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(session.staged[0].tags, vec!["drum"]);
    }

    #[test]
    fn render_includes_non_empty_text_and_status_rows() {
        let session = SampleImportSession {
            staged: vec![ready_sample("kick.wav")],
            selected: 0,
            batch_tags: vec!["drum".to_string()],
            tag_candidates: Vec::new(),
            edit: None,
            pending_control_c: false,
        };
        let text = session.render_text(DEFAULT_WRAP_WIDTH).text;
        assert!(text.contains("Sample Import"));
        assert!(text.contains("ready: 1"));
        assert!(text.contains("> 001"));
    }

    #[test]
    fn render_wraps_long_sample_rows() {
        let mut sample = ready_sample("/Users/example/samples/very/deep/folder/Minha Gente.wav");
        sample.tags = vec![
            "brazilian".to_string(),
            "dreamy".to_string(),
            "erasmo carlos".to_string(),
        ];
        let session = SampleImportSession {
            staged: vec![sample],
            selected: 0,
            batch_tags: Vec::new(),
            tag_candidates: Vec::new(),
            edit: None,
            pending_control_c: false,
        };
        let rendered = session.render_text(52);
        assert!(
            rendered.text.lines().all(|line| line.chars().count() <= 52),
            "wrapped text should fit width:\n{}",
            rendered.text
        );
    }

    #[test]
    fn commit_requires_two_consecutive_control_c_keys() {
        let mut session = SampleImportSession {
            staged: vec![ready_sample("kick.wav")],
            selected: 0,
            batch_tags: Vec::new(),
            tag_candidates: Vec::new(),
            edit: None,
            pending_control_c: false,
        };
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(session.handle_key(ctrl_c), ImportKeyOutcome::Handled);
        assert_eq!(session.handle_key(ctrl_c), ImportKeyOutcome::Commit);
    }

    #[test]
    fn control_c_release_does_not_complete_commit_chord() {
        let mut session = SampleImportSession {
            staged: vec![ready_sample("kick.wav")],
            selected: 0,
            batch_tags: Vec::new(),
            tag_candidates: Vec::new(),
            edit: None,
            pending_control_c: false,
        };
        let press = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let release = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        };
        assert_eq!(session.handle_key(press), ImportKeyOutcome::Handled);
        assert_eq!(session.handle_key(release), ImportKeyOutcome::Handled);
        assert_eq!(session.handle_key(press), ImportKeyOutcome::Commit);
    }
}
