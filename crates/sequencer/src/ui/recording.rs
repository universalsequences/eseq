use std::time::Instant;

use crossterm::event::KeyCode;

use crate::recorder::{default_recording_name, resolve_recording_path, save_recording_wav};

use super::{App, InputMode};

impl App {
    pub(super) fn toggle_master_recording(&mut self) {
        if self.ui.master_recording {
            match self.master_recorder.stop() {
                Ok(take) => {
                    self.ui.master_recording = false;
                    if take.samples.is_empty() {
                        self.editor.status_message =
                            Some(("Recording is empty".to_string(), Instant::now()));
                        return;
                    }
                    let frames = take.samples.len() / usize::from(take.channels.max(1));
                    let mut status = format!(
                        "Captured {:.2}s of master audio",
                        frames as f32 / take.sample_rate as f32
                    );
                    if take.dropped_blocks > 0 {
                        status.push_str(&format!(" (dropped {} block(s))", take.dropped_blocks));
                    }
                    self.pending_recording_take = Some(take);
                    self.ui.value_buffer = default_recording_name();
                    self.ui.input_mode = InputMode::WavExportNameEntry;
                    self.editor.status_message = Some((status, Instant::now()));
                }
                Err(error) => {
                    self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
                }
            }
            return;
        }

        match self.master_recorder.start() {
            Ok(()) => {
                self.ui.master_recording = true;
                self.editor.status_message =
                    Some(("Recording master output".to_string(), Instant::now()));
            }
            Err(error) => {
                self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
            }
        }
    }

    pub(super) fn handle_wav_export_name_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => self.ui.value_buffer.push(c),
            KeyCode::Backspace => {
                self.ui.value_buffer.pop();
            }
            KeyCode::Enter => {
                let requested = self.ui.value_buffer.trim().to_string();
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
                let Some(take) = self.pending_recording_take.take() else {
                    self.editor.status_message =
                        Some(("No pending recording to save".to_string(), Instant::now()));
                    return;
                };
                let path = resolve_recording_path(&requested);
                match save_recording_wav(&path, &take) {
                    Ok(()) => {
                        let mut msg = format!("Saved recording to {}", path.display());
                        if take.dropped_blocks > 0 {
                            msg.push_str(&format!(" ({} dropped block(s))", take.dropped_blocks));
                        }
                        self.editor.status_message = Some((msg, Instant::now()));
                    }
                    Err(error) => {
                        self.editor.status_message =
                            Some((format!("Error: {error}"), Instant::now()));
                    }
                }
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
                self.pending_recording_take = None;
            }
            _ => {}
        }
    }
}
