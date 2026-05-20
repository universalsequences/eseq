use super::*;
use eseqlisp::widget_render::number_picker::{
    clear_number_picker_edit_state, handle_number_picker_edit_key_for_widget,
    number_picker_edit_state, NumberPickerEditOutcome,
};

#[derive(Clone, Debug)]
pub(crate) struct HeldKeyboardNote {
    key: char,
    transpose: f32,
    step_at_press: usize,
    press_time: Instant,
    tracks: Vec<usize>,
}

pub(crate) fn layout_node_by_id(
    node: &eseqlisp::layout::LayoutNode,
    id: u64,
) -> Option<&eseqlisp::layout::LayoutNode> {
    if node.widget_id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = layout_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

pub(crate) fn layout_node_by_stable_key<'a>(
    node: &'a eseqlisp::layout::LayoutNode,
    stable_key: &str,
    widget_type: Option<&str>,
) -> Option<&'a eseqlisp::layout::LayoutNode> {
    if node.stable_key.as_deref() == Some(stable_key)
        && widget_type.is_none_or(|expected| node.widget_type == expected)
    {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = layout_node_by_stable_key(child, stable_key, widget_type) {
            return Some(found);
        }
    }
    None
}

pub(crate) fn focused_widget_matches(
    editor: &Editor,
    predicate: impl FnOnce(&str) -> bool,
) -> bool {
    let Some(focused_id) = editor.focused_widget_id() else {
        return false;
    };
    let Some(layout) = editor.widget_layout() else {
        return false;
    };
    let Some(node) = layout_node_by_id(&layout, focused_id) else {
        return false;
    };
    predicate(node.widget_type.as_str())
}

pub(crate) fn focused_widget_captures_space(editor: &Editor) -> bool {
    focused_widget_matches(editor, widget_type_captures_text_input)
}

pub(crate) fn focused_widget_captures_text_input(editor: &Editor) -> bool {
    focused_widget_matches(editor, widget_type_captures_text_input)
}

fn widget_type_captures_text_input(widget_type: &str) -> bool {
    matches!(widget_type, "text-input" | "textbox" | "patcher")
}

fn active_buffer_accepts_global_step_shortcuts(editor: &Editor) -> bool {
    let buffer = editor.active_buffer();
    if buffer.name == "*piano-roll*" {
        return false;
    }
    matches!(buffer.view_mode, ViewMode::UiOnly)
}

fn active_buffer_accepts_global_ui_shortcuts(editor: &Editor) -> bool {
    matches!(editor.active_buffer().view_mode, ViewMode::UiOnly)
}

fn is_toggle_mods_view_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    if key.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }

    match key.code {
        KeyCode::Char('m') | KeyCode::Char('M') => key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER),
        _ => false,
    }
}

pub(crate) fn held_note_for_key(
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    key: &crossterm::event::KeyEvent,
) -> bool {
    let c = match key.code {
        crossterm::event::KeyCode::Char(c) => c.to_ascii_lowercase(),
        _ => return false,
    };
    held_notes.lock().unwrap().iter().any(|note| note.key == c)
}

pub(crate) fn should_route_to_live_keyboard(
    editor: &Editor,
    key: &crossterm::event::KeyEvent,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
) -> bool {
    use crossterm::event::{KeyEventKind, KeyModifiers};

    if matches!(key.kind, KeyEventKind::Release) {
        return held_note_for_key(held_notes, key);
    }

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return false;
    }

    if focused_widget_captures_text_input(editor) {
        return false;
    }

    matches!(key.code, crossterm::event::KeyCode::Char(_))
}

pub(crate) fn normalize_command_shortcuts(
    key: crossterm::event::KeyEvent,
) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    if matches!(
        key.code,
        KeyCode::Char('a')
            | KeyCode::Char('A')
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Char('v')
            | KeyCode::Char('V')
    ) && key.modifiers.contains(KeyModifiers::SUPER)
    {
        let mut modifiers = key.modifiers;
        modifiers.remove(KeyModifiers::SUPER);
        modifiers.insert(KeyModifiers::CONTROL);
        return KeyEvent::new(key.code, modifiers);
    }

    key
}

pub(crate) fn should_toggle_play_on_space(
    editor: &Editor,
    key: &crossterm::event::KeyEvent,
) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if key.code != KeyCode::Char(' ') || key.modifiers != KeyModifiers::NONE {
        return false;
    }

    if editor.minibuffer_prompt().is_some() || focused_widget_captures_space(editor) {
        return false;
    }

    let buffer = editor.active_buffer();
    buffer.read_only || matches!(buffer.view_mode, ViewMode::UiOnly) || buffer.name == "*metal*"
}

pub(crate) fn should_reload_custom_ui_after_key(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(key.code, KeyCode::Char('b') | KeyCode::Char('B'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(crate) fn current_metal_cursor_step(editor: &mut Editor) -> Option<usize> {
    match editor.runtime_mut().eval_str("(current-step)") {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

pub(crate) fn current_metal_param_mode(editor: &mut Editor) -> Option<usize> {
    match editor.runtime_mut().eval_str("param-mode") {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::widget_type_captures_text_input;

    #[test]
    fn patcher_captures_text_input_shortcuts() {
        assert!(widget_type_captures_text_input("text-input"));
        assert!(widget_type_captures_text_input("textbox"));
        assert!(widget_type_captures_text_input("patcher"));
        assert!(!widget_type_captures_text_input("button"));
    }
}

pub(crate) fn metal_has_selected_bus(editor: &mut Editor) -> bool {
    matches!(
        editor.runtime_mut().eval_str("(seq-has-selected-bus?)"),
        Ok(Some(Value::Bool(true)))
    )
}

fn metal_step_param_for_mode(mode: usize) -> Option<StepParam> {
    match mode {
        0 => Some(StepParam::Velocity),
        1 => Some(StepParam::Duration),
        2 => Some(StepParam::AuxA),
        3 => Some(StepParam::Transpose),
        4 => Some(StepParam::Pan),
        // Sync is rendered as a label in the step footer, not a numeric picker.
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SoftStepParamEditTarget {
    track: usize,
    step: usize,
    param: StepParam,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SoftStepParamEdit {
    target: Option<SoftStepParamEditTarget>,
    widget_id: Option<u64>,
    editing: bool,
}

impl SoftStepParamEdit {
    fn is_active(&self) -> bool {
        self.target.is_some() && self.widget_id.is_some() && self.editing
    }

    fn clear(&mut self) {
        if let Some(widget_id) = self.widget_id {
            clear_number_picker_edit_state(widget_id);
        }
        self.target = None;
        self.widget_id = None;
        self.editing = false;
    }
}

fn current_soft_step_param_target(
    editor: &mut Editor,
    current_track: &Arc<AtomicUsize>,
) -> Option<SoftStepParamEditTarget> {
    if editor.active_buffer().name != "*metal*" || metal_has_selected_bus(editor) {
        return None;
    }
    let step = current_metal_cursor_step(editor)?;
    let mode = current_metal_param_mode(editor)?;
    let param = metal_step_param_for_mode(mode)?;
    Some(SoftStepParamEditTarget {
        track: current_track.load(Ordering::Relaxed),
        step,
        param,
    })
}

fn current_metal_step_param_number_picker_id(editor: &Editor) -> Option<u64> {
    let layout = editor.widget_layout()?;
    layout_node_by_stable_key(
        &layout,
        "metal-step-param-number-picker",
        Some("number-picker"),
    )
    .map(|node| node.widget_id)
}

fn numeric_edit_char(key: &crossterm::event::KeyEvent) -> Option<char> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let KeyCode::Char(c) = key.code else {
        return None;
    };
    if !(c.is_ascii_digit() || c == '.' || c == '-') {
        return None;
    }
    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        Some(c)
    } else {
        None
    }
}

fn number_picker_soft_edit_key(key: &crossterm::event::KeyEvent, edit_active: bool) -> bool {
    use crossterm::event::KeyCode;

    numeric_edit_char(key).is_some()
        || (edit_active && matches!(key.code, KeyCode::Backspace | KeyCode::Enter | KeyCode::Esc))
}

fn starts_unarmed_number_picker_edit(key: &crossterm::event::KeyEvent) -> bool {
    numeric_edit_char(key).is_some()
}

/// Route only numeric text-editing keys to the current Metal step parameter.
///
/// This deliberately avoids real widget focus so arrow keys can keep their
/// sequencer meaning. The edit buffer mirrors number-picker semantics: first
/// numeric key starts a fresh value, Enter commits, Esc cancels.
pub(crate) fn handle_metal_soft_step_param_key(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    edit: &mut SoftStepParamEdit,
) -> bool {
    use crossterm::event::KeyEventKind;

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if focused_widget_captures_text_input(editor) {
        return false;
    }

    if !number_picker_soft_edit_key(key, edit.is_active()) {
        if edit.is_active() {
            edit.clear();
            editor.mark_needs_redraw();
        }
        return false;
    }

    if numeric_edit_char(key).is_some() {
        if !edit.is_active() && !starts_unarmed_number_picker_edit(key) {
            return false;
        }
        let Some(target) = current_soft_step_param_target(editor, current_track) else {
            return false;
        };
        let Some(widget_id) = current_metal_step_param_number_picker_id(editor) else {
            return false;
        };
        if edit.target != Some(target) || edit.widget_id != Some(widget_id) {
            edit.clear();
            edit.target = Some(target);
            edit.widget_id = Some(widget_id);
            edit.editing = false;
        }
    }

    let Some(target) = edit.target else {
        return false;
    };
    let Some(widget_id) = edit.widget_id else {
        return false;
    };
    let current_value = state.pattern.step_data[target.track].get(target.step, target.param);
    let decimals = if target.param == StepParam::Transpose {
        0
    } else {
        2
    };
    let outcome = handle_number_picker_edit_key_for_widget(
        widget_id,
        eseqlisp::widget_render::WidgetKeyEvent {
            code: key.code,
            modifiers: key.modifiers,
        },
        current_value as f64,
        target.param.min() as f64,
        target.param.max() as f64,
        decimals,
    );

    match outcome {
        Some(NumberPickerEditOutcome::StateChanged) => {
            edit.editing = number_picker_edit_state(widget_id).editing;
            if !edit.editing {
                edit.clear();
            }
            editor.mark_needs_redraw();
            true
        }
        Some(NumberPickerEditOutcome::Commit(value)) => {
            state.pattern.step_data[target.track].set(target.step, target.param, value as f32);
            state.publish_scheduler_snapshot();
            sync_step_param_lists(editor.runtime_mut(), state, target.track);
            edit.clear();
            editor.mark_needs_redraw();
            true
        }
        None if edit.is_active() => {
            edit.clear();
            editor.mark_needs_redraw();
            true
        }
        None => false,
    }
}

pub(crate) fn handle_metal_command_shortcut(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    step_clipboard: &Arc<Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>>,
) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && !focused_widget_captures_text_input(editor)
        && is_toggle_mods_view_shortcut(key)
    {
        let _ = editor
            .runtime_mut()
            .eval_str("(seq-toggle-current-track-mods-view)");
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
        return true;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && active_buffer_accepts_global_ui_shortcuts(editor)
        && editor.focused_widget_id().is_none()
        && !focused_widget_captures_text_input(editor)
    {
        match (key.code, key.modifiers) {
            (KeyCode::Tab, KeyModifiers::CONTROL) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(seq-toggle-piano-roll-placement)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Tab, KeyModifiers::NONE) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(seq-toggle-main-or-piano-roll)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Tab, KeyModifiers::SHIFT) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(seq-toggle-metal-sequencer-main)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Up | KeyCode::Down, KeyModifiers::NONE)
                if editor.active_buffer().name != "*piano-roll*" =>
            {
                let track_count = state.active_track_count();
                if track_count == 0 {
                    return true;
                }
                let current = current_track.load(Ordering::Relaxed).min(track_count - 1);
                let next = if key.code == KeyCode::Up {
                    if current == 0 {
                        track_count - 1
                    } else {
                        current - 1
                    }
                } else {
                    (current + 1) % track_count
                };
                let _ = editor.runtime_mut().eval_str(&format!(
                    "(do (set! selected-bus -1) (seq-set-track {next}))"
                ));
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ => {}
        }
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'))
    {
        let _ = editor.runtime_mut().eval_str("(agent-open-instrument)");
        editor.refresh_runtime_side_effects();
        return true;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && active_buffer_accepts_global_step_shortcuts(editor)
        && editor.focused_widget_id().is_none()
        && !focused_widget_captures_text_input(editor)
    {
        match (key.code, key.modifiers) {
            (KeyCode::Left, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(cursor-left)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(cursor-right)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Backspace | KeyCode::Delete, KeyModifiers::NONE) => {
                if selected_steps.lock().unwrap().is_empty() {
                    return false;
                }
                let _ = editor.runtime_mut().eval_str("(delete-selected-steps)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ => {}
        }
    } else {
        return false;
    }

    if key.modifiers.contains(KeyModifiers::SUPER) {
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') => {
                let track = current_track.load(Ordering::Relaxed);
                let steps: Vec<usize> = {
                    let set = selected_steps.lock().unwrap();
                    if set.is_empty() {
                        current_metal_cursor_step(editor).into_iter().collect()
                    } else {
                        let mut steps: Vec<usize> = set.iter().copied().collect();
                        steps.sort_unstable();
                        steps
                    }
                };
                if steps.is_empty() {
                    return true;
                }
                let anchor = steps[0];
                let clipboard: Vec<(usize, sequencer::sequencer::StepSnapshot)> = steps
                    .iter()
                    .map(|&s| (s - anchor, state.capture_step_snapshot(track, s)))
                    .collect();
                let count = clipboard.len();
                *step_clipboard.lock().unwrap() = Some((track, clipboard));
                editor.handle_host_event(HostEvent::Status(format!(
                    "Copied {} step{}",
                    count,
                    if count == 1 { "" } else { "s" }
                )));
                return true;
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                let dest_start = match current_metal_cursor_step(editor) {
                    Some(step) => step,
                    None => return true,
                };
                let clipboard = {
                    let guard = step_clipboard.lock().unwrap();
                    guard.clone()
                };
                let Some((source_track, clipboard)) = clipboard else {
                    return true;
                };
                let track = current_track.load(Ordering::Relaxed);
                let preserve_audio_plocks = source_track == track;
                let num_steps = state.pattern.track_params[track].get_num_steps();
                for (offset, snapshot) in &clipboard {
                    let dest = dest_start + offset;
                    if dest >= num_steps {
                        continue;
                    }
                    if !snapshot.active && state.pattern.patterns[track].is_active(dest) {
                        continue;
                    }
                    let sanitized = if preserve_audio_plocks {
                        snapshot.clone()
                    } else {
                        snapshot.without_audio_plocks()
                    };
                    state.restore_step_snapshot(track, dest, &sanitized);
                }
                state.publish_scheduler_snapshot();
                editor.handle_host_event(HostEvent::Status(format!(
                    "Pasted {} step{}",
                    clipboard.len(),
                    if clipboard.len() == 1 { "" } else { "s" }
                )));
                return true;
            }
            _ => {}
        }
    }

    false
}

/// Map keyboard character to semitone offset (piano-style layout).
pub(crate) fn note_from_key(c: char) -> Option<i32> {
    match c {
        'a' => Some(0),  // C
        'w' => Some(1),  // C#
        's' => Some(2),  // D
        'e' => Some(3),  // D#
        'd' => Some(4),  // E
        'f' => Some(5),  // F
        't' => Some(6),  // F#
        'g' => Some(7),  // G
        'y' => Some(8),  // G#
        'h' => Some(9),  // A
        'u' => Some(10), // A#
        'j' => Some(11), // B
        'k' => Some(12), // C+1
        'o' => Some(13), // C#+1
        'l' => Some(14), // D+1
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingKeyOutcome {
    Ignored,
    Consumed,
    Recorded,
}

impl RecordingKeyOutcome {
    pub(crate) fn consumed(self) -> bool {
        !matches!(self, Self::Ignored)
    }

    pub(crate) fn recorded(self) -> bool {
        matches!(self, Self::Recorded)
    }
}

/// Intercept keyboard events for live recording.
pub(crate) fn handle_recording_key(
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    recording: &Arc<AtomicBool>,
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    keyboard_octave: &Arc<std::sync::atomic::AtomicI32>,
    current_track: &Arc<AtomicUsize>,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    ui_epoch: &Arc<AtomicUsize>,
) -> RecordingKeyOutcome {
    use crossterm::event::{KeyCode, KeyEventKind};

    let c = match key.code {
        KeyCode::Char(c) => c.to_ascii_lowercase(),
        _ => return RecordingKeyOutcome::Ignored,
    };

    // Octave shift keys (only on press)
    if c == 'z' || c == 'x' {
        if key.kind == KeyEventKind::Press {
            let delta = if c == 'z' { -12 } else { 12 };
            keyboard_octave.fetch_add(delta, Ordering::Relaxed);
        }
        return RecordingKeyOutcome::Consumed;
    }

    let note = match note_from_key(c) {
        Some(n) => n,
        None => return RecordingKeyOutcome::Ignored,
    };

    match key.kind {
        KeyEventKind::Press => {
            // Suppress key repeat — only trigger on first press
            let mut held = held_notes.lock().unwrap();
            if held.iter().any(|note| note.key == c) {
                return RecordingKeyOutcome::Consumed;
            }

            let armed = record_armed.lock().unwrap();
            let octave = keyboard_octave.load(Ordering::Relaxed);
            let transpose = (note + octave) as f32;
            let mut pressed_tracks = Vec::new();

            // Send note-on to audio thread for all armed tracks
            for (track, a) in armed.iter().enumerate() {
                if *a {
                    pressed_tracks.push(track);
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track,
                        transpose,
                        velocity: 1.0,
                        note_off: false,
                    });
                }
            }

            // Record the step at press time
            let ct = current_track.load(Ordering::Relaxed);
            let playhead = state.transport.track_playheads[ct].load(Ordering::Relaxed) as usize;
            held.push(HeldKeyboardNote {
                key: c,
                transpose,
                step_at_press: playhead,
                press_time: Instant::now(),
                tracks: pressed_tracks,
            });
            RecordingKeyOutcome::Consumed
        }
        KeyEventKind::Release => {
            // Find and remove the held note
            let held_entry = {
                let mut held = held_notes.lock().unwrap();
                let pos = held.iter().position(|note| note.key == c);
                pos.map(|idx| held.remove(idx))
            };

            // Record into pattern if recording + playing
            if let Some(note) = held_entry {
                for track in &note.tracks {
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track: *track,
                        transpose: note.transpose,
                        velocity: 0.0,
                        note_off: true,
                    });
                }

                if recording.load(Ordering::Relaxed) && state.is_playing() {
                    let armed = record_armed.lock().unwrap();
                    let bpm = state.transport.bpm.load(Ordering::Relaxed) as f64;
                    let secs_per_step = 60.0 / bpm / 4.0;
                    let hold_secs = note.press_time.elapsed().as_secs_f64();
                    let duration_steps = (hold_secs / secs_per_step).max(0.15).min(64.0) as f32;
                    let mut recorded = false;

                    for (track, a) in armed.iter().enumerate() {
                        if !*a {
                            continue;
                        }
                        let num_steps = state.pattern.track_params[track].get_num_steps();
                        let local_step = note.step_at_press % num_steps;
                        if !state.pattern.patterns[track].is_active(local_step) {
                            state.pattern.patterns[track].toggle_step(local_step);
                        }
                        state.pattern.chord_data[track].add_note(local_step, note.transpose);
                        let first_note = state.pattern.chord_data[track].get(local_step, 0);
                        state.pattern.step_data[track].set(
                            local_step,
                            StepParam::Transpose,
                            first_note,
                        );
                        state.pattern.step_data[track].set(local_step, StepParam::Velocity, 1.0);
                        state.pattern.step_data[track].set(
                            local_step,
                            StepParam::Duration,
                            duration_steps,
                        );
                        recorded = true;
                    }
                    if recorded {
                        state.publish_scheduler_snapshot();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        return RecordingKeyOutcome::Recorded;
                    }
                }
            }
            RecordingKeyOutcome::Consumed
        }
        _ => RecordingKeyOutcome::Consumed, // consume Repeat events too
    }
}

#[cfg(test)]
mod live_keyboard_tests {
    use super::{
        handle_metal_command_shortcut, held_note_for_key, note_from_key, HeldKeyboardNote,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use eseqlisp::editor::ViewMode;
    use eseqlisp::{Editor, EditorConfig, Runtime};
    use sequencer::sequencer::{SequencerState, StepSnapshot};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[test]
    fn held_note_lookup_is_case_insensitive_for_release_matching() {
        let held = Arc::new(Mutex::new(vec![HeldKeyboardNote {
            key: 'a',
            transpose: 0.0,
            step_at_press: 0,
            press_time: Instant::now(),
            tracks: vec![0],
        }]));
        let key = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);

        assert!(held_note_for_key(&held, &key));
    }

    #[test]
    fn live_note_map_uses_lowercase_keys() {
        assert_eq!(note_from_key('a'), Some(0));
        assert_eq!(note_from_key('A'), None);
    }

    #[test]
    fn up_down_switch_tracks_outside_piano_roll() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str("(defstate selected-bus -1)")
            .expect("install selected bus state");
        let current_track = Arc::new(AtomicUsize::new(0));
        let native_track = Arc::clone(&current_track);
        editor
            .runtime_mut()
            .register_native("seq-set-track", move |args, _ctx| {
                let Some(eseqlisp::vm::Value::Number(track)) = args.first() else {
                    return Err("expected track".to_string());
                };
                native_track.store(*track as usize, Ordering::Relaxed);
                Ok(eseqlisp::vm::Value::Number(*track))
            });
        let state = Arc::new(SequencerState::new(3, vec![]));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(current_track.load(Ordering::Relaxed), 1);

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(current_track.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn tab_shortcuts_call_main_panel_toggles() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate tab-target "")
                (def seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
                (def seq-toggle-metal-sequencer-main () (set! tab-target "sequencer"))
                (def seq-toggle-piano-roll-placement () (set! tab-target "placement"))
                "#,
            )
            .expect("install tab handlers");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("piano".to_string()))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("sequencer".to_string()))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("placement".to_string()))
        );
    }

    #[test]
    fn command_or_control_m_toggles_current_track_mods_view() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate instrument-panel-tab 2)
                (defstate instrument-mods-open false)
                (defstate selected-bus 1)
                (def instrument-toggle-mods-view ()
                  (do
                    (set! instrument-panel-tab 0)
                    (set! instrument-mods-open (not instrument-mods-open))))
                (def seq-toggle-current-track-mods-view ()
                  (do
                    (set! selected-bus -1)
                    (instrument-toggle-mods-view)))
                "#,
            )
            .expect("install mods view handler");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('m'), KeyModifiers::SUPER),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("instrument-mods-open")
                .unwrap(),
            Some(eseqlisp::vm::Value::Bool(true))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("instrument-panel-tab")
                .unwrap(),
            Some(eseqlisp::vm::Value::Number(0.0))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("selected-bus").unwrap(),
            Some(eseqlisp::vm::Value::Number(-1.0))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("instrument-mods-open")
                .unwrap(),
            Some(eseqlisp::vm::Value::Bool(false))
        );
    }

    #[test]
    fn command_m_toggles_mods_view_outside_ui_buffers() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().read_only = true;
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate instrument-mods-open false)
                (def instrument-toggle-mods-view ()
                  (set! instrument-mods-open (not instrument-mods-open)))
                (def seq-toggle-current-track-mods-view ()
                  (instrument-toggle-mods-view))
                "#,
            )
            .expect("install mods view handler");
        editor.clear_needs_redraw();
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('m'), KeyModifiers::SUPER),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("instrument-mods-open")
                .unwrap(),
            Some(eseqlisp::vm::Value::Bool(true))
        );
        assert!(
            editor.needs_redraw(),
            "mods shortcut should schedule a frame after mutating Lisp UI state"
        );
    }

    #[test]
    fn global_ui_shortcuts_do_not_run_in_command_menu() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate tab-target "")
                (defstate cursor-left-count 0)
                (def seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
                (def cursor-left () (set! cursor-left-count (+ cursor-left-count 1)))
                "#,
            )
            .expect("install tab handler");
        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert!(editor.minibuffer_prompt().is_some());

        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("".to_string()))
        );

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("cursor-left-count").unwrap(),
            Some(eseqlisp::vm::Value::Number(0.0))
        );
    }

    #[test]
    fn global_ui_shortcuts_do_not_run_in_editable_buffers() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate tab-target "")
                (def seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
                "#,
            )
            .expect("install tab handler");

        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("".to_string()))
        );
    }

    #[test]
    fn global_ui_shortcuts_do_not_run_in_read_only_text_buffers() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().read_only = true;
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate tab-target "")
                (def seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
                "#,
            )
            .expect("install tab handler");
        let current_track = Arc::new(AtomicUsize::new(0));
        let native_track = Arc::clone(&current_track);
        editor
            .runtime_mut()
            .register_native("seq-set-track", move |args, _ctx| {
                let Some(eseqlisp::vm::Value::Number(track)) = args.first() else {
                    return Err("expected track".to_string());
                };
                native_track.store(*track as usize, Ordering::Relaxed);
                Ok(eseqlisp::vm::Value::Number(*track))
            });
        let state = Arc::new(SequencerState::new(3, vec![]));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(current_track.load(Ordering::Relaxed), 0);

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("".to_string()))
        );
    }
}
