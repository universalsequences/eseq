use super::*;
use eseqlisp::ui::platform::{
    clipboard_shortcut_modifier_for, has_primary_shortcut_modifier,
    has_primary_shortcut_modifier_for, has_sequencer_shortcut_modifier,
    is_exact_primary_shortcut_modifier, is_exact_primary_shortcut_modifier_for,
    ShortcutPlatform, CURRENT_SHORTCUT_PLATFORM,
};
use eseqlisp::widget_render::number_picker::{
    clear_number_picker_edit_state, handle_number_picker_edit_key_for_widget,
    number_picker_edit_state, NumberPickerEditOutcome,
};

#[derive(Clone, Debug)]
pub(crate) struct HeldKeyboardNote {
    key: char,
    sequence_roll_code: Option<crossterm::event::KeyCode>,
    transpose: f32,
    press_time: Instant,
    /// Everything this key press sounds on, with the record position each
    /// target was pressed at. One entry per armed track, plus the rack pad
    /// target when a drum rack is armed.
    targets: Vec<LiveNoteTarget>,
}

/// One live-keyboard target of a held key: the track it sounds on, at the
/// transpose it sounds at, from the record position of the press. Armed tracks
/// play chromatically (the key's own transpose); with a drum rack armed the
/// matching pad plays its member track at base pitch — transpose 0 — so the
/// member's own base note decides the pitch (docs/drum-rack-v2-spec.md,
/// "Arming & live play").
#[derive(Clone, Copy, Debug)]
pub(crate) struct LiveNoteTarget {
    track: usize,
    transpose: f32,
    position: sequencer::sequencer::RecordPosition,
    /// True once `position` has been replaced by the audio thread's
    /// record-as-heard stamp for this trigger (bead eseq-2awi); until then it
    /// holds the wall-clock press estimate as a fallback.
    stamped: bool,
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
    predicate: impl FnOnce(&eseqlisp::layout::LayoutNode) -> bool,
) -> bool {
    editor
        .focused_widget_node()
        .is_some_and(|node| predicate(&node))
}

pub(crate) fn focused_widget_captures_space(editor: &Editor) -> bool {
    focused_widget_matches(editor, widget_captures_text_input)
}

pub(crate) fn focused_widget_captures_text_input(editor: &Editor) -> bool {
    focused_widget_matches(editor, widget_captures_text_input)
}

fn widget_type_captures_text_input(widget_type: &str) -> bool {
    matches!(widget_type, "text-input" | "textbox")
}

fn widget_captures_text_input(node: &eseqlisp::layout::LayoutNode) -> bool {
    widget_type_captures_text_input(node.widget_type.as_str())
        || eseqlisp::widget_render::patcher::patcher_has_text_edit(node)
}

fn focused_number_picker_is_editing(editor: &Editor) -> bool {
    editor.focused_widget_node().is_some_and(|node| {
        node.widget_type == "number-picker"
            && number_picker_edit_state(node.widget_id).editing
    })
}

/// Whether the editor currently permits sequencer live-keyboard shortcuts.
///
/// Two ways in. Text-bearing buffers must opt in through their major mode, so
/// source and special text modes keep ownership of their bare keys. Widget-only
/// buffers (`ViewMode::UiOnly` — piano roll, browser, arrangement, transport…)
/// are admitted without an opt-in: they have no text pane, so there is nothing
/// for the keys to be stolen from, and requiring each to declare a mode meant
/// every GUI buffer that never called `set-buffer-mode-for` silently went mute.
///
/// Either way focus supersedes mode: prompts and focused text or numeric
/// editors retain ownership of keys, including inside a UiOnly buffer.
fn editor_accepts_live_keyboard_input(editor: &Editor) -> bool {
    (editor.active_mode_accepts_live_keys() || active_buffer_accepts_global_ui_shortcuts(editor))
        && editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && !focused_widget_captures_text_input(editor)
        && !focused_number_picker_is_editing(editor)
}

/// A focused patcher owns Cmd+Z/Cmd+Shift+Z for its graph-level undo history,
/// so the app-level sequencer history shortcut must let the key fall through
/// to the widget key path.
fn focused_widget_is_patcher(editor: &Editor) -> bool {
    focused_widget_matches(editor, |node| node.widget_type == "patcher")
}

fn active_buffer_accepts_global_ui_shortcuts(editor: &Editor) -> bool {
    matches!(editor.active_buffer().view_mode, ViewMode::UiOnly)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SequencerHistoryShortcut {
    Undo,
    Redo,
}

pub(crate) fn sequencer_history_shortcut(
    editor: &Editor,
    key: &crossterm::event::KeyEvent,
) -> Option<SequencerHistoryShortcut> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    if !matches!(key.kind, KeyEventKind::Press)
        || !matches!(key.code, KeyCode::Char('z' | 'Z'))
        || editor.minibuffer_prompt().is_some()
        || editor.prompt_text().is_some()
        || !active_buffer_accepts_global_ui_shortcuts(editor)
        || focused_widget_captures_text_input(editor)
        || focused_widget_is_patcher(editor)
    {
        return None;
    }

    if !has_primary_shortcut_modifier(key.modifiers)
        || key.modifiers.contains(KeyModifiers::ALT)
    {
        return None;
    }
    Some(if key.modifiers.contains(KeyModifiers::SHIFT) {
        SequencerHistoryShortcut::Redo
    } else {
        SequencerHistoryShortcut::Undo
    })
}

fn global_sequencer_navigation_available(editor: &Editor) -> bool {
    editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && active_buffer_accepts_global_ui_shortcuts(editor)
        && editor.focused_widget_id().is_none()
        && !focused_widget_captures_text_input(editor)
}

fn fx_plock_row_selected(editor: &mut Editor) -> bool {
    let Some(callable) = editor.runtime_mut().global_value("eseq.effects.track-panels/plock-row-selected?") else {
        return false;
    };
    matches!(
        editor.runtime_mut().invoke(callable, vec![]),
        Ok(Some(Value::Bool(true)))
    )
}

fn selected_steps_delete_shortcut_available(
    editor: &mut Editor,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    let has_selected_steps = !selected_steps.lock().unwrap().is_empty();
    has_selected_steps && !fx_plock_row_selected(editor)
}

fn select_track_for_edit(editor: &mut Editor, track: usize) {
    if let Some(callable) = editor
        .runtime_mut()
        .global_value("eseq.sequencer/select-track-for-edit")
    {
        let _ = editor
            .runtime_mut()
            .invoke(callable, vec![Value::Number(track as f64)]);
    } else {
        let _ = editor.runtime_mut().eval_str(&format!(
            "(do (set! eseq.seq-core-state/selected-bus -1) (seq-set-track {track}))"
        ));
    }
}

fn select_track_relative(
    editor: &mut Editor,
    current: usize,
    delta: isize,
    track_count: usize,
) {
    if let Some(callable) = editor
        .runtime_mut()
        .global_value("eseq.drum-rack-v2/track-relative")
    {
        let next = editor.runtime_mut().invoke(
            callable,
            vec![
                Value::Number(current as f64),
                Value::Number(delta as f64),
            ],
        );
        if let Ok(Some(Value::Number(next))) = next {
            if next.is_finite() && next >= 0.0 && next.fract() == 0.0 && next < track_count as f64 {
                select_track_for_edit(editor, next as usize);
            }
        }
        return;
    }

    let next = if delta < 0 {
        if current == 0 { track_count - 1 } else { current - 1 }
    } else {
        (current + 1) % track_count
    };
    select_track_for_edit(editor, next);
}

fn focused_widget_is(editor: &Editor, stable_key: &str, widget_type: &str) -> bool {
    editor.focused_widget_node().is_some_and(|node| {
        node.stable_key.as_deref() == Some(stable_key) && node.widget_type == widget_type
    })
}

fn shortcut_context_allows_sample_browser_focus(editor: &Editor) -> bool {
    editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && editor.active_buffer().view_mode != ViewMode::TextOnly
        && !focused_widget_captures_text_input(editor)
}

fn shortcut_context_allows_sequencer_tab_switch(editor: &Editor) -> bool {
    editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && editor.active_buffer().view_mode != ViewMode::TextOnly
        && !focused_widget_captures_text_input(editor)
}

fn sequencer_tab_shortcut_index(key: &crossterm::event::KeyEvent) -> Option<usize> {
    sequencer_tab_shortcut_index_for(key, CURRENT_SHORTCUT_PLATFORM)
}

fn sequencer_tab_shortcut_index_for(
    key: &crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> Option<usize> {
    if !is_exact_primary_shortcut_modifier_for(key.modifiers, platform) {
        return None;
    }
    let crossterm::event::KeyCode::Char(ch) = key.code else {
        return None;
    };
    ch.to_digit(10)
        .and_then(|digit| usize::try_from(digit).ok())
        .filter(|index| (1..=9).contains(index))
}

fn current_sequencer_step_tab_buffer(editor: &mut Editor) -> String {
    match editor.runtime_mut().eval_str("(eseq.seq-panels/seq-current-step-buffer)") {
        Ok(Some(Value::String(buffer))) => buffer,
        _ => "*sequencer*".to_string(),
    }
}

fn select_sequencer_tab_by_index(editor: &mut Editor, index: usize) -> bool {
    let current_buffer = current_sequencer_step_tab_buffer(editor);
    if !editor.switch_active_tile_to_buffer_named(&current_buffer)
        && (current_buffer == "*sequencer*"
            || !editor.switch_active_tile_to_buffer_named("*sequencer*"))
    {
        return false;
    }

    let selected = matches!(
        editor
            .runtime_mut()
            .eval_str(&format!("(eseq.seq-step-tabs/seq-select-main-step-tab-by-index {index})")),
        Ok(Some(Value::Bool(true)))
    );
    if selected {
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
    selected
}

/// `ui/browser.lisp` is `(module eseq.browser)`, so its widget `:key`s
/// auto-qualify (module spec §10 hazard a) and the search input hashes as
/// `eseq.browser/search-input`. The flat spelling is still live in the
/// `live_keyboard_tests` stub browser below, which is a headerless eval, so both
/// are tried. Drop the legacy one when that stub qualifies too.
const BROWSER_SEARCH_INPUT_KEY: &str = "eseq.browser/search-input";
const LEGACY_BROWSER_SEARCH_INPUT_KEY: &str = "sbrowser-search-input";

fn focus_samples_browser_search(editor: &mut Editor) -> bool {
    if !editor.switch_active_tile_to_buffer_named("*samples*") {
        if let Some(callable) = editor.runtime_mut().global_value("eseq.browser/sample-browser-here") {
            let _ = editor.runtime_mut().invoke(callable, vec![]);
        } else {
            let _ = editor
                .runtime_mut()
                .eval_str(r#"(switch-to-buffer "*samples*")"#);
        }
        editor.refresh_runtime_side_effects();
    }
    editor.refresh_visible_layouts_for_buffer_named("*samples*");
    if editor.active_buffer().name != "*samples*" {
        return false;
    }
    editor.focus_widget_by_stable_key(BROWSER_SEARCH_INPUT_KEY, Some("text-input"))
        || editor.focus_widget_by_stable_key(LEGACY_BROWSER_SEARCH_INPUT_KEY, Some("text-input"))
}

/// `ui/transport.lisp` is `(module eseq.transport)`, so its widget `:key`s
/// auto-qualify (module spec §10 hazard a) and the BPM picker hashes as
/// `eseq.transport/transport-bpm`. The bare spelling is tried second so a
/// headerless eval of the transport UI (tests, scratch) still resolves.
const TRANSPORT_BPM_PICKER_KEY: &str = "eseq.transport/transport-bpm";
const BARE_TRANSPORT_BPM_PICKER_KEY: &str = "transport-bpm";

fn focus_transport_bpm(editor: &mut Editor) -> bool {
    if !editor.switch_active_tile_to_buffer_named("*transport*") {
        let _ = editor
            .runtime_mut()
            .eval_str(r#"(switch-to-buffer "*transport*")"#);
        editor.refresh_runtime_side_effects();
    }
    editor.refresh_visible_layouts_for_buffer_named("*transport*");
    if editor.active_buffer().name != "*transport*" {
        return false;
    }
    editor.focus_widget_by_stable_key(TRANSPORT_BPM_PICKER_KEY, Some("number-picker"))
        || editor.focus_widget_by_stable_key(BARE_TRANSPORT_BPM_PICKER_KEY, Some("number-picker"))
}

fn sample_browser_active_tree_key(editor: &mut Editor) -> Option<String> {
    match editor.runtime_mut().eval_str("(eseq.browser/active-tree-key)") {
        Ok(Some(Value::String(key))) => Some(key),
        _ => None,
    }
}

fn focus_samples_browser_active_tree(editor: &mut Editor) -> bool {
    let Some(tree_key) = sample_browser_active_tree_key(editor) else {
        return false;
    };
    editor.refresh_visible_layouts_for_buffer_named("*samples*");
    editor.focus_widget_by_stable_key(&tree_key, Some("tree"))
}

fn focused_samples_search_should_hand_off_to_tree(
    editor: &Editor,
    key: &crossterm::event::KeyEvent,
) -> bool {
    editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && editor.active_buffer().name == "*samples*"
        && key.modifiers == crossterm::event::KeyModifiers::NONE
        && matches!(
            key.code,
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Down
        )
        && (focused_widget_is(editor, BROWSER_SEARCH_INPUT_KEY, "text-input")
            || focused_widget_is(editor, LEGACY_BROWSER_SEARCH_INPUT_KEY, "text-input"))
}

fn sample_browser_search_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    sample_browser_search_shortcut_for(key, CURRENT_SHORTCUT_PLATFORM)
}

fn sample_browser_search_shortcut_for(
    key: &crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> bool {
    use crossterm::event::KeyCode;

    matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F'))
        && is_exact_primary_shortcut_modifier_for(key.modifiers, platform)
}

fn is_toggle_mods_view_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    if key.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }

    match key.code {
        KeyCode::Char('m') | KeyCode::Char('M') => {
            has_sequencer_shortcut_modifier(key.modifiers)
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PatternLengthShortcut {
    Double,
    Halve,
}

fn pattern_length_shortcut(key: &crossterm::event::KeyEvent) -> Option<PatternLengthShortcut> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let has_shortcut_modifier = has_sequencer_shortcut_modifier(key.modifiers);
    if !has_shortcut_modifier || key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }

    match key.code {
        KeyCode::Char('+') | KeyCode::Char('=') => Some(PatternLengthShortcut::Double),
        KeyCode::Char('-') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(PatternLengthShortcut::Halve)
        }
        _ => None,
    }
}

fn is_trigger_recording_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::KeyCode;

    key.code == KeyCode::Char(',') && is_exact_primary_shortcut_modifier(key.modifiers)
}

/// Cmd+B (macOS) / Ctrl+B (elsewhere) — jump to the transport BPM picker.
/// Only claims the platform-primary chord: text-editing Ctrl+B (move-left,
/// and the custom-UI reload piggybacked on it) is preserved by the TextOnly
/// gate at the dispatch site.
fn is_focus_bpm_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    matches!(
        key.code,
        crossterm::event::KeyCode::Char('b') | crossterm::event::KeyCode::Char('B')
    ) && is_exact_primary_shortcut_modifier(key.modifiers)
}

/// Cmd+R (macOS) / Ctrl+R (elsewhere) — arm the current track exclusively
/// (disarming every other track); pressing again disarms it.
fn is_exclusive_arm_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    matches!(
        key.code,
        crossterm::event::KeyCode::Char('r') | crossterm::event::KeyCode::Char('R')
    ) && is_exact_primary_shortcut_modifier(key.modifiers)
}

fn is_new_instrument_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    is_new_instrument_shortcut_for(key, CURRENT_SHORTCUT_PLATFORM)
}

fn is_new_instrument_shortcut_for(
    key: &crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> bool {
    use crossterm::event::KeyCode;

    matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I'))
        && is_exact_primary_shortcut_modifier_for(key.modifiers, platform)
}

fn is_duplicate_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    is_duplicate_shortcut_for(key, CURRENT_SHORTCUT_PLATFORM)
}

fn is_duplicate_shortcut_for(
    key: &crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> bool {
    use crossterm::event::KeyCode;

    matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D'))
        && is_exact_primary_shortcut_modifier_for(key.modifiers, platform)
}

fn is_plain_tab_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(
        (key.code, key.modifiers),
        (KeyCode::Tab, KeyModifiers::NONE)
    )
}

/// Inside the patch editor Tab is autocomplete-only: letting it fall through
/// to the session/arrangement toggle silently discards in-progress patch
/// edits, so consume it instead.
fn tab_locked_to_patch_editor(editor: &mut Editor) -> bool {
    if focused_widget_matches(editor, |node| node.widget_type == "patcher") {
        return true;
    }
    matches!(
        editor.runtime_mut().eval_str("eseq.seq-step-tabs/seq-layout-mode"),
        Ok(Some(Value::Keyword(mode)))
            if mode == "instrument-patcher"
                || mode == "instrument-patcher-source"
                || mode == "instrument-patcher-learn"
    )
}

fn is_shift_tab_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(
        (key.code, key.modifiers),
        (KeyCode::Tab, KeyModifiers::SHIFT)
            | (KeyCode::BackTab, KeyModifiers::SHIFT)
            | (KeyCode::BackTab, KeyModifiers::NONE)
    )
}

pub(crate) fn held_note_for_key(
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    key: &crossterm::event::KeyEvent,
) -> bool {
    let normalized_code = match key.code {
        crossterm::event::KeyCode::Char(c) => {
            crossterm::event::KeyCode::Char(c.to_ascii_lowercase())
        }
        code => code,
    };
    held_notes.lock().unwrap().iter().any(|note| {
        note.sequence_roll_code == Some(normalized_code)
            || matches!(normalized_code, crossterm::event::KeyCode::Char(c) if note.key == c)
    })
}

/// Identify transport-level roll-rate key candidates independently of track
/// arming. `should_route_to_live_keyboard` applies the active-mode and editor
/// focus gate before a candidate can change the roll rate.
pub(crate) fn is_active_roll_rate_key(
    state: &SequencerState,
    key: &crossterm::event::KeyEvent,
) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind};

    state.transport.roll_mode.load(Ordering::Relaxed)
        && key.kind == KeyEventKind::Press
        && key.modifiers.is_empty()
        && matches!(
            key.code,
            KeyCode::Char(c) if sequencer::sequencer::Timebase::roll_rate_from_key(c).is_some()
        )
}

pub(crate) fn should_route_to_live_keyboard(
    editor: &Editor,
    key: &crossterm::event::KeyEvent,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    semantic_hold_binding: bool,
) -> bool {
    use crossterm::event::{KeyEventKind, KeyModifiers};

    if matches!(key.kind, KeyEventKind::Release) {
        return held_note_for_key(held_notes, key);
    }

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if !semantic_hold_binding
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return false;
    }

    if !editor_accepts_live_keyboard_input(editor) {
        return false;
    }

    // A focused number picker or knob-number begins its edit on the first
    // digit. Roll-rate keys are digits, so without this they eat the keypress
    // that would have started the edit — and `editor_accepts_live_keyboard_input`
    // only notices a picker that is *already* editing, which is one key too late.
    if !semantic_hold_binding
        && matches!(key.code, crossterm::event::KeyCode::Char(c) if c.is_ascii_digit())
        && editor.focused_widget_captures_numeric_input()
    {
        return false;
    }

    semantic_hold_binding || matches!(key.code, crossterm::event::KeyCode::Char(_))
}

pub(crate) fn normalize_command_shortcuts(
    key: crossterm::event::KeyEvent,
) -> crossterm::event::KeyEvent {
    normalize_command_shortcuts_for(key, CURRENT_SHORTCUT_PLATFORM)
}

fn normalize_command_shortcuts_for(
    key: crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    // The editor's select-all command is represented internally as Ctrl+A.
    // Only macOS input needs translating; Linux's platform-primary chord is
    // already Ctrl+A. Global UI dispatch sees the raw event before this step.
    // Clipboard chords (Cmd-C/Cmd-V) are NOT translated: editor and widget
    // handlers understand the platform-native modifier directly.
    if platform == ShortcutPlatform::MacOS
        && matches!(key.code, KeyCode::Char('a') | KeyCode::Char('A'))
        && has_primary_shortcut_modifier_for(key.modifiers, platform)
    {
        let mut modifiers = key.modifiers;
        modifiers.remove(eseqlisp::ui::platform::primary_shortcut_modifier_for(platform));
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
    match editor.runtime_mut().eval_str("(eseq.seq-core-state/current-step)") {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

pub(crate) fn current_metal_param_mode(editor: &mut Editor) -> Option<usize> {
    match editor.runtime_mut().eval_str("eseq.seq-core-state/param-mode") {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

fn current_sequencer_cursor_step(editor: &mut Editor) -> Option<usize> {
    match editor
        .runtime_mut()
        .eval_str("(eseq.sequencer/current-selected-step)")
    {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

fn current_sequencer_param_mode(editor: &mut Editor) -> Option<usize> {
    match editor.runtime_mut().eval_str("(eseq.sequencer/current-param-mode)") {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{normalize_command_shortcuts, widget_type_captures_text_input};

    #[test]
    fn command_clipboard_shortcuts_keep_the_native_modifier_for_text_buffers() {
        for key in ['c', 'v'] {
            let event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::SUPER);
            assert_eq!(normalize_command_shortcuts(event), event);
        }
    }

    #[test]
    fn only_text_entry_widgets_capture_text_input_by_type() {
        assert!(widget_type_captures_text_input("text-input"));
        assert!(widget_type_captures_text_input("textbox"));
        assert!(!widget_type_captures_text_input("patcher"));
        assert!(!widget_type_captures_text_input("button"));
    }
}

pub(crate) fn metal_has_selected_bus(editor: &mut Editor) -> bool {
    matches!(
        editor.runtime_mut().eval_str("(eseq.seq-core-state/seq-has-selected-bus?)"),
        Ok(Some(Value::Bool(true)))
    )
}

fn metal_selected_drum_rack_bus(editor: &mut Editor) -> Option<usize> {
    match editor.runtime_mut().eval_str(
        "(if (>= (eseq.drum-rack-v2/rack-of-bus eseq.seq-core-state/selected-bus) 0) \
           eseq.seq-core-state/selected-bus -1)",
    ) {
        Ok(Some(Value::Number(bus))) if bus >= 0.0 => Some(bus as usize),
        _ => None,
    }
}

fn metal_step_param_for_mode(mode: usize) -> Option<StepParam> {
    match mode {
        0 => Some(StepParam::Velocity),
        1 => Some(StepParam::Duration),
        2 => Some(StepParam::AuxA),
        3 => Some(StepParam::Transpose),
        4 => Some(StepParam::Pan),
        6 => Some(StepParam::Delay),
        // Sync is rendered as a label in the step footer, not a numeric picker.
        _ => None,
    }
}

fn metal_mode_for_step_param(param: StepParam) -> Option<usize> {
    match param {
        StepParam::Velocity => Some(0),
        StepParam::Duration => Some(1),
        StepParam::AuxA => Some(2),
        StepParam::Transpose => Some(3),
        StepParam::Pan => Some(4),
        StepParam::Sync => Some(5),
        StepParam::Delay => Some(6),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SoftStepParamEditKind {
    StepParam(StepParam),
    ProcessLane {
        instance_id: sequencer::process::ProcessInstanceId,
        inlet_name: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SoftStepParamEditTarget {
    track: usize,
    step: usize,
    kind: SoftStepParamEditKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SoftStepParamEditSpec {
    value: f32,
    min: f32,
    max: f32,
    decimals: u8,
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

fn soft_step_param_edit_spec(
    state: &Arc<SequencerState>,
    target: &SoftStepParamEditTarget,
) -> Option<SoftStepParamEditSpec> {
    match &target.kind {
        SoftStepParamEditKind::StepParam(param) => Some(SoftStepParamEditSpec {
            value: state.pattern.step_data[target.track].get(target.step, *param),
            min: param.min(),
            max: param.max(),
            decimals: if *param == StepParam::Transpose { 0 } else { 2 },
        }),
        SoftStepParamEditKind::ProcessLane {
            instance_id,
            inlet_name,
        } => {
            let lane = process_lane_edit_info_for_target(
                state,
                target.track,
                *instance_id,
                inlet_name,
                target.step,
            )?;
            Some(SoftStepParamEditSpec {
                value: lane.value,
                min: lane.min,
                max: lane.max,
                decimals: lane.decimals,
            })
        }
    }
}

fn sync_soft_process_lane_commit(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    target: &SoftStepParamEditTarget,
) {
    let runtime = editor.runtime_mut();
    sync_process_chain_state(
        runtime,
        state,
        state.active_track_count(),
        current_track.load(Ordering::Relaxed),
    );
    for viewport in expanded_step_projection.viewports_for_track(target.track) {
        if let Some(slot) = visible_slot_for_step(viewport, target.step) {
            let _ = sync_expanded_step_param_slot(runtime, state, viewport, viewport.mode, slot);
        }
    }
}

fn sync_soft_step_param_commit(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    target: &SoftStepParamEditTarget,
    param: StepParam,
) {
    let runtime = editor.runtime_mut();
    sync_step_param_lists(runtime, state, target.track);
    if let Some(mode) = metal_mode_for_step_param(param) {
        for viewport in expanded_step_projection.viewports_for_track(target.track) {
            if let Some(slot) = visible_slot_for_step(viewport, target.step) {
                let _ = sync_expanded_step_param_slot(runtime, state, viewport, mode, slot);
            }
        }
    }
}

fn commit_soft_step_param_edit(
    editor: &mut Editor,
    app: &mut app::App,
    current_track: &Arc<AtomicUsize>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    target: &SoftStepParamEditTarget,
    value: f64,
) -> bool {
    match &target.kind {
        SoftStepParamEditKind::StepParam(param) => {
            if app::try_apply_command(
                app,
                app::AppCommand::SetStepParam {
                    track: target.track,
                    step: target.step,
                    param: *param,
                    value: value as f32,
                },
            )
            .is_err()
            {
                return false;
            }
            sync_soft_step_param_commit(
                editor,
                &app.state,
                expanded_step_projection,
                target,
                *param,
            );
            true
        }
        SoftStepParamEditKind::ProcessLane {
            instance_id,
            inlet_name,
        } => {
            let result = app.apply_recorded_scene_structure_mutation(
                "Edit process lane",
                |app| app.state.set_process_lane_value(
                    target.track,
                    *instance_id,
                    inlet_name.clone(),
                    target.step,
                    value as f32,
                ).then_some(()).ok_or_else(|| "Process lane target is missing or unchanged".to_string()),
            );
            if result.is_err() {
                return false;
            }
            sync_soft_process_lane_commit(
                editor,
                &app.state,
                current_track,
                expanded_step_projection,
                target,
            );
            true
        }
    }
}

fn current_soft_step_param_target(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
) -> Option<SoftStepParamEditTarget> {
    if metal_has_selected_bus(editor) {
        return None;
    }
    let buffer_name = editor.active_buffer().name.clone();
    let (step, mode) = match buffer_name.as_str() {
        "*metal*" => (
            current_metal_cursor_step(editor)?,
            current_metal_param_mode(editor)?,
        ),
        "*sequencer*" => (
            current_sequencer_cursor_step(editor)?,
            current_sequencer_param_mode(editor)?,
        ),
        _ => return None,
    };
    if let Some(param) = metal_step_param_for_mode(mode) {
        return Some(SoftStepParamEditTarget {
            track: current_track.load(Ordering::Relaxed),
            step,
            kind: SoftStepParamEditKind::StepParam(param),
        });
    }
    let track = current_track.load(Ordering::Relaxed);
    if buffer_name == "*sequencer*" {
        let lane = process_lane_edit_info_for_mode(state, track, mode, step)?;
        return Some(SoftStepParamEditTarget {
            track,
            step,
            kind: SoftStepParamEditKind::ProcessLane {
                instance_id: lane.instance_id,
                inlet_name: lane.inlet_name,
            },
        });
    }
    None
}

fn current_step_param_number_picker_key(editor: &mut Editor) -> Option<String> {
    let buffer_name = editor.active_buffer().name.clone();
    match buffer_name.as_str() {
        // ui/step-grid.lisp is `eseq.step-grid` since S3b wave 8, so its
        // widget `:key` auto-qualifies (spec §10 hazard a/l). The *metal*
        // buffer is not created by the live UI any more (editor_setup.rs),
        // but the spelling has to track the lisp side regardless.
        "*metal*" => Some("eseq.step-grid/step-param-number-picker".to_string()),
        "*sequencer*" => match editor
            .runtime_mut()
            .eval_str("(eseq.sequencer/current-number-picker-key)")
        {
            Ok(Some(Value::String(key))) => Some(key),
            _ => None,
        },
        _ => None,
    }
}

fn current_step_param_number_picker_id(editor: &mut Editor) -> Option<u64> {
    let key = current_step_param_number_picker_key(editor)?;
    let layout = editor.widget_layout()?;
    layout_node_by_stable_key(&layout, key.as_str(), Some("number-picker"))
        .map(|node| node.widget_id)
}

fn current_step_param_number_picker_is_editing(editor: &mut Editor) -> bool {
    current_step_param_number_picker_id(editor)
        .is_some_and(|widget_id| number_picker_edit_state(widget_id).editing)
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

fn number_picker_pending_edit_key(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::KeyCode;

    matches!(key.code, KeyCode::Backspace | KeyCode::Enter | KeyCode::Esc)
}

fn starts_unarmed_number_picker_edit(key: &crossterm::event::KeyEvent) -> bool {
    numeric_edit_char(key).is_some()
}

/// Route only numeric text-editing keys to the current Metal step parameter or
/// expanded sequencer process lane.
///
/// This deliberately avoids real widget focus so arrow keys can keep their
/// sequencer meaning. The edit buffer mirrors number-picker semantics: first
/// numeric key starts a fresh value, Enter commits, Esc cancels.
pub(crate) fn handle_metal_soft_step_param_key(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    app: &mut app::App,
    current_track: &Arc<AtomicUsize>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    edit: &mut SoftStepParamEdit,
) -> bool {
    use crossterm::event::KeyEventKind;

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if focused_widget_captures_text_input(editor) {
        return false;
    }

    let soft_edit_key = number_picker_soft_edit_key(key, edit.is_active());
    if !soft_edit_key {
        let pending_edit_key = number_picker_pending_edit_key(key);
        if pending_edit_key {
            if let (Some(target), Some(widget_id)) = (
                current_soft_step_param_target(editor, &app.state, current_track),
                current_step_param_number_picker_id(editor),
            ) {
                if number_picker_edit_state(widget_id).editing {
                    edit.target = Some(target);
                    edit.widget_id = Some(widget_id);
                    edit.editing = true;
                }
            }
        }

        if !pending_edit_key || !edit.is_active() {
            if edit.is_active() {
                edit.clear();
                editor.mark_needs_redraw();
            }
            return false;
        }
    }

    if numeric_edit_char(key).is_some() {
        if !edit.is_active() && !starts_unarmed_number_picker_edit(key) {
            return false;
        }
        let Some(target) = current_soft_step_param_target(editor, &app.state, current_track) else {
            return false;
        };
        let Some(widget_id) = current_step_param_number_picker_id(editor) else {
            return false;
        };
        if edit.target.as_ref() != Some(&target) || edit.widget_id != Some(widget_id) {
            edit.clear();
            edit.target = Some(target);
            edit.widget_id = Some(widget_id);
            edit.editing = false;
        }
    }

    let Some(target) = edit.target.clone() else {
        return false;
    };
    let Some(widget_id) = edit.widget_id else {
        return false;
    };
    let Some(spec) = soft_step_param_edit_spec(&app.state, &target) else {
        edit.clear();
        editor.mark_needs_redraw();
        return true;
    };
    let outcome = handle_number_picker_edit_key_for_widget(
        widget_id,
        eseqlisp::widget_render::WidgetKeyEvent {
            code: key.code,
            modifiers: key.modifiers,
        },
        spec.value as f64,
        spec.min as f64,
        spec.max as f64,
        u32::from(spec.decimals),
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
            if !commit_soft_step_param_edit(
                editor,
                app,
                current_track,
                expanded_step_projection,
                &target,
                value,
            ) {
                edit.clear();
                editor.mark_needs_redraw();
                return true;
            }
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
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

/// Give an explicit major-mode clipboard binding ownership before the editor's
/// global text clipboard binding. Focused text widgets remain first-class text
/// editors and therefore keep the same chord for themselves.
fn handle_mode_clipboard_shortcut(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
) -> bool {
    use crossterm::event::KeyCode;

    if editor.minibuffer_prompt().is_some()
        || editor.prompt_text().is_some()
        || focused_widget_captures_text_input(editor)
        || key.modifiers != clipboard_shortcut_modifier_for(CURRENT_SHORTCUT_PLATFORM)
        || !matches!(key.code, KeyCode::Char('c' | 'C' | 'v' | 'V'))
    {
        return false;
    }
    let Some(handler) = editor.active_mode_keybinding(*key).map(str::to_string) else {
        return false;
    };
    if let Err(error) = editor.runtime_mut().invoke_global(&handler, Vec::new()) {
        editor.handle_host_event(HostEvent::Status(format!(
            "Could not run clipboard shortcut: {error:?}"
        )));
    }
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    true
}

/// Arrangement (Arr tab) region clipboard seam (region spec 5.3).
///
/// The region and the edit cursor are Rust-owned but published as
/// `SEQ.song-region` / `SEQ.song-bound-clip`, which is the single source of
/// truth this reads — no second mirror to drift. The actual work happens in
/// the region host commands, where the clipboard handle is in scope.
fn arrangement_view_is_active(editor: &Editor) -> bool {
    editor.active_buffer().name == "*arrangement*"
}

/// Whether a published `SEQ.*` value is set (non-nil).
fn published_value_is_set(editor: &mut Editor, expr: &str) -> bool {
    editor
        .runtime_mut()
        .eval_str(expr)
        .ok()
        .flatten()
        .is_some_and(|value| !matches!(value, eseqlisp::vm::Value::Nil))
}

fn enqueue_region_command(editor: &mut Editor, name: &str) {
    editor
        .runtime_mut()
        .enqueue_host_command(HostCommand::Custom {
            name: name.to_string(),
            payload: Value::Nil,
        });
}

/// Platform-primary C / V / D, or Backspace, over the arrangement. Returns
/// true when the key was consumed.
///
/// Backspace only takes the key for a MARQUEE region (region set, no clip
/// selected). A clip click also sets a one-clip region (spec 4.1 as amended),
/// and that case must keep falling through to the existing clip-delete path.
fn handle_arrangement_region_shortcut(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
) -> bool {
    handle_arrangement_region_shortcut_for(editor, key, CURRENT_SHORTCUT_PLATFORM)
}

fn handle_arrangement_region_shortcut_for(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !arrangement_view_is_active(editor)
        || editor.minibuffer_prompt().is_some()
        || editor.prompt_text().is_some()
        || focused_widget_captures_text_input(editor)
    {
        return false;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('c') | KeyCode::Char('C'), modifiers)
            if modifiers == clipboard_shortcut_modifier_for(platform) =>
        {
            if !published_value_is_set(editor, "SEQ.song-region") {
                return false;
            }
            enqueue_region_command(editor, "song-region-copy");
            true
        }
        (KeyCode::Char('v') | KeyCode::Char('V'), modifiers)
            if modifiers == clipboard_shortcut_modifier_for(platform) =>
        {
            // No payload: the command pastes at the mirrored arrangement
            // cursor, floored to the clipboard's own grid. An empty clipboard
            // reports on the status line rather than falling through to a
            // shortcut that means nothing here.
            enqueue_region_command(editor, "song-region-paste");
            true
        }
        (KeyCode::Char('d') | KeyCode::Char('D'), modifiers)
            if has_primary_shortcut_modifier_for(modifiers, platform) =>
        {
            if !published_value_is_set(editor, "SEQ.song-region") {
                return false;
            }
            enqueue_region_command(editor, "song-region-duplicate");
            true
        }
        (KeyCode::Backspace | KeyCode::Delete, KeyModifiers::NONE) => {
            if !published_value_is_set(editor, "SEQ.song-region")
                || published_value_is_set(editor, "SEQ.song-bound-clip")
            {
                return false;
            }
            enqueue_region_command(editor, "song-region-delete");
            true
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn handle_metal_command_shortcut(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    step_clipboard: &Arc<Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>>,
) -> bool {
    let ui_epoch = AtomicUsize::new(0);
    handle_metal_command_shortcut_with_ui_epoch(
        editor,
        key,
        state,
        current_track,
        selected_steps,
        step_clipboard,
        &ui_epoch,
    )
}

pub(crate) fn handle_metal_command_shortcut_with_ui_epoch(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    step_clipboard: &Arc<Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>>,
    _ui_epoch: &AtomicUsize,
) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    // The main event loop routes modal key presses directly to the editor.
    // Keep this lower-level entry point safe for other callers and tests: no
    // sequencer command may execute while a modal owns keyboard input.
    if editor.modal_is_open() {
        return false;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && is_trigger_recording_shortcut(key)
    {
        let _ = editor.runtime_mut().eval_str("(seq-toggle-record)");
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
        return true;
    }

    if shortcut_context_allows_sequencer_tab_switch(editor) && is_focus_bpm_shortcut(key) {
        return focus_transport_bpm(editor);
    }

    if shortcut_context_allows_sequencer_tab_switch(editor) && is_exclusive_arm_shortcut(key) {
        let track = current_track.load(Ordering::Relaxed);
        let _ = editor
            .runtime_mut()
            .eval_str(&format!("(seq-arm-track-exclusive {track})"));
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
        return true;
    }

    if number_picker_pending_edit_key(key) && current_step_param_number_picker_is_editing(editor) {
        return false;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && !focused_widget_captures_text_input(editor)
        && is_toggle_mods_view_shortcut(key)
    {
        let _ = editor
            .runtime_mut()
            .eval_str("(eseq.seq-panels/seq-toggle-current-track-mods-view)");
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
        return true;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && !focused_widget_captures_text_input(editor)
        && is_shift_tab_shortcut(key)
        && editor
            .runtime_mut()
            .eval_str(r#"(or (= SEQ.editor-mode "new-instrument") (= SEQ.editor-mode "edit-instrument"))"#)
            .ok()
            .flatten()
            .is_some_and(|value| matches!(value, eseqlisp::vm::Value::Bool(true)))
    {
        let _ = editor
            .runtime_mut()
            .eval_str("(eseq.seq-panels/seq-toggle-main-or-piano-roll)");
        editor.refresh_runtime_side_effects();
        return true;
    }

    if shortcut_context_allows_sequencer_tab_switch(editor) {
        if let Some(index) = sequencer_tab_shortcut_index(key) {
            return select_sequencer_tab_by_index(editor, index);
        }
    }

    if shortcut_context_allows_sample_browser_focus(editor) && sample_browser_search_shortcut(key) {
        return focus_samples_browser_search(editor);
    }

    if focused_samples_search_should_hand_off_to_tree(editor, key) {
        return focus_samples_browser_active_tree(editor);
    }

    if handle_mode_clipboard_shortcut(editor, key) {
        return true;
    }

    if handle_arrangement_region_shortcut(editor, key) {
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
        return true;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
        && key.modifiers == KeyModifiers::NONE
        // Defer to a focused widget (e.g. an arrangement lane with a selected
        // clip, a text input, a number picker mid-edit) only when that widget
        // actually handles this key. Gating on "anything is focused" meant a
        // click on any button — which consumes only Enter and Space — silently
        // disarmed Cmd+A followed by Backspace.
        && !editor.focused_widget_consumes_key(key.code, key.modifiers)
        && selected_steps_delete_shortcut_available(editor, selected_steps)
    {
        let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/delete-selected-steps)");
        editor.refresh_runtime_side_effects();
        return true;
    }

    if global_sequencer_navigation_available(editor) {
        match (key.code, key.modifiers) {
            (KeyCode::Left, KeyModifiers::SHIFT) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-select-left)");
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            (KeyCode::Right, KeyModifiers::SHIFT) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-select-right)");
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            (KeyCode::Left, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-left)");
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-right)");
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-toggle)");
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            (KeyCode::Up | KeyCode::Down, KeyModifiers::NONE) => {
                let track_count = state.active_track_count();
                if track_count == 0 {
                    return true;
                }
                let current = current_track.load(Ordering::Relaxed).min(track_count - 1);
                let delta = if key.code == KeyCode::Up { -1 } else { 1 };
                select_track_relative(editor, current, delta, track_count);
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            _ => {}
        }
    }

    let vim_normal_plain_tab = is_plain_tab_shortcut(key)
        && editor.active_vim_input_mode() == Some(eseqlisp::editor::VimInputMode::Normal);
    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && (active_buffer_accepts_global_ui_shortcuts(editor) || vim_normal_plain_tab)
        && !focused_widget_captures_text_input(editor)
    {
        if let Some(shortcut) = pattern_length_shortcut(key) {
            let command = if metal_has_selected_bus(editor) {
                let Some(bus) = metal_selected_drum_rack_bus(editor) else {
                    return false;
                };
                let operation = match shortcut {
                    PatternLengthShortcut::Double => "double",
                    PatternLengthShortcut::Halve => "halve",
                };
                format!("(seq-resize-drum-rack-patterns {bus} :{operation})")
            } else {
                match shortcut {
                    PatternLengthShortcut::Double => {
                        "(eseq.seq-grid-mode/double-track-pattern)".to_string()
                    }
                    PatternLengthShortcut::Halve => {
                        "(eseq.seq-grid-mode/halve-track-pattern)".to_string()
                    }
                }
            };
            let _ = editor.runtime_mut().eval_str(&command);
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
            return true;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('a') | KeyCode::Char('A'), modifiers)
                if has_sequencer_shortcut_modifier(modifiers)
                    && !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                let command = if editor.active_buffer().name == "*sequencer*" {
                    "eseq.sequencer/select-all-current-track-steps"
                } else {
                    "eseq.step-grid-interactions/select-all-steps"
                };
                let _ = editor.runtime_mut().invoke_global(command, Vec::new());
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            (KeyCode::Tab, KeyModifiers::CONTROL) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(eseq.seq-panels/seq-toggle-piano-roll-placement)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ if is_plain_tab_shortcut(key) => {
                if tab_locked_to_patch_editor(editor) {
                    return true;
                }
                if editor.active_vim_input_mode() == Some(eseqlisp::editor::VimInputMode::Insert) {
                    return false;
                }
                let _ = if let Some(callable) = editor
                    .runtime_mut()
                    .global_value("eseq.seq-panels/seq-toggle-arrangement")
                {
                    editor.runtime_mut().invoke(callable, vec![])
                } else {
                    editor.runtime_mut().eval_str("(eseq.seq-panels/seq-toggle-arrangement)")
                };
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ if is_shift_tab_shortcut(key) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(eseq.seq-panels/seq-toggle-main-or-piano-roll)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ if editor.focused_widget_id().is_some() => {}
            (KeyCode::Char('h') | KeyCode::Char('H'), KeyModifiers::CONTROL) => {
                let _ = editor.runtime_mut().eval_str("(eseq.sequencer/collapse-all-tracks)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ if is_new_instrument_shortcut(key) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(host-command \"enter-new-instrument-editor\" (dict))");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ if is_duplicate_shortcut(key) && editor.active_buffer().name == "*mixer*" => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(seq-clone-active-track-pattern)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ => {}
        }
    }

    if has_sequencer_shortcut_modifier(key.modifiers)
        && matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'))
    {
        // The platform-primary chord owns the global track-group dispatcher.
        let _ = editor.runtime_mut().eval_str("(eseq.mixer/seq-ctrl-g)");
        editor.refresh_runtime_side_effects();
        return true;
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && active_buffer_accepts_global_ui_shortcuts(editor)
        && editor.focused_widget_id().is_none()
        && !focused_widget_captures_text_input(editor)
    {
        match (key.code, key.modifiers) {
            (KeyCode::Left, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-left)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/cursor-right)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Backspace | KeyCode::Delete, KeyModifiers::NONE) => {
                if !selected_steps_delete_shortcut_available(editor, selected_steps) {
                    return false;
                }
                let _ = editor.runtime_mut().eval_str("(eseq.step-grid-interactions/delete-selected-steps)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ => {}
        }
    } else {
        return false;
    }

    if let Some(shortcut) = step_clipboard_shortcut_for(key, CURRENT_SHORTCUT_PLATFORM) {
        match shortcut {
            StepClipboardShortcut::Copy => {
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
            StepClipboardShortcut::Paste => {
                let dest_start = match current_metal_cursor_step(editor) {
                    Some(step) => step,
                    None => return true,
                };
                if step_clipboard.lock().unwrap().is_none() {
                    return true;
                }
                let track = current_track.load(Ordering::Relaxed);
                let mut payload = HashMap::new();
                payload.insert(
                    "track".to_string(),
                    Rc::new(RefCell::new(Value::Number(track as f64))),
                );
                payload.insert(
                    "dest-start".to_string(),
                    Rc::new(RefCell::new(Value::Number(dest_start as f64))),
                );
                editor.runtime_mut().enqueue_host_command(HostCommand::Custom {
                    name: "paste-steps".to_string(),
                    payload: Value::Map(payload),
                });
                return true;
            }
        }
    }

    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepClipboardShortcut {
    Copy,
    Paste,
}

fn step_clipboard_shortcut_for(
    key: &crossterm::event::KeyEvent,
    platform: ShortcutPlatform,
) -> Option<StepClipboardShortcut> {
    use crossterm::event::KeyCode;

    if key.modifiers != clipboard_shortcut_modifier_for(platform) {
        return None;
    }
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('C') => Some(StepClipboardShortcut::Copy),
        KeyCode::Char('v') | KeyCode::Char('V') => Some(StepClipboardShortcut::Paste),
        _ => None,
    }
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
    /// A live key press fired at least one armed target. The host drops any
    /// lingering widget focus (e.g. a number picker) so the player's next
    /// keys keep reaching the live keyboard instead of the widget.
    Triggered,
    Recorded,
    /// Notes were written into a pending take (takes spec 8.4): the live
    /// pattern is untouched, so no step-grid rebuild and no live-recording
    /// history transaction.
    RecordedTake,
}

impl RecordingKeyOutcome {
    pub(crate) fn consumed(self) -> bool {
        !matches!(self, Self::Ignored)
    }

    pub(crate) fn triggered_note(self) -> bool {
        matches!(self, Self::Triggered)
    }

    pub(crate) fn recorded(self) -> bool {
        matches!(self, Self::Recorded)
    }

    pub(crate) fn recorded_take(self) -> bool {
        matches!(self, Self::RecordedTake)
    }
}

/// Convert a captured local position to its recorded step and per-note delay.
/// Coarser grids are expressed in beats and then converted through the
/// track's timebase, which keeps recording quantization independent from the
/// global transport resolution.
fn quantized_record_position(
    step: usize,
    phase: f32,
    num_steps: usize,
    timebase: sequencer::sequencer::Timebase,
    quantize: sequencer::record_quantize::RecordQuantize,
) -> (usize, f32) {
    let num_steps = num_steps.max(1);
    let phase = phase.clamp(0.0, 1.0);
    match quantize {
        sequencer::record_quantize::RecordQuantize::Off => (step % num_steps, phase),
        sequencer::record_quantize::RecordQuantize::Sixteenth => {
            ((step + usize::from(phase >= 0.5)) % num_steps, 0.0)
        }
        _ => {
            let grid_beats = quantize
                .grid_beats()
                .expect("non-off record quantization must define a grid");
            let grid_steps = (grid_beats / timebase.step_beats(num_steps)).max(1.0e-9);
            let snapped = ((step as f64 + phase as f64) / grid_steps).round() * grid_steps;
            (snapped.round().rem_euclid(num_steps as f64) as usize, 0.0)
        }
    }
}

/// Record position a live key press lands on for `track`. The latency
/// compensated record clock is authoritative; a track with no anchor yet
/// (stopped transport) falls back to its published playhead.
fn press_record_position(
    state: &Arc<SequencerState>,
    track: usize,
    press_time: Instant,
) -> sequencer::sequencer::RecordPosition {
    state
        .record_position_at_instant(track, press_time)
        .unwrap_or_else(|| sequencer::sequencer::RecordPosition {
            step: state.transport.track_playheads[track].load(Ordering::Relaxed) as usize,
            phase: f32::from_bits(
                state.transport.track_playhead_phases[track].load(Ordering::Relaxed),
            )
            .clamp(0.0, 1.0),
        })
}

/// Reposition held live-note targets from the audio thread's note-on stamps
/// (bead eseq-2awi, record-as-heard). Each stamp carries the render-timeline
/// beat the trigger actually sounded at; resolving it through the same
/// track-local geometry the scheduler replays it with replaces the target's
/// wall-clock press estimate — killing input-delivery jitter and the
/// device/PDC latency guesswork in one move. Called from the per-frame
/// reactive tick and again at note release, so the stamp has normally landed
/// before the release writes the step; a trigger whose stamp has not arrived
/// yet (a sub-block tap) keeps its press-estimate fallback.
pub(crate) fn apply_live_trigger_stamps(
    state: &Arc<SequencerState>,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
) {
    let debug = record_stamp_debug();
    let mut held = held_notes.lock().unwrap();
    state.drain_live_trigger_stamps(|stamp| {
        let Some(position) = state.record_position_at_beat(stamp.track, stamp.beat) else {
            return;
        };
        // First unstamped match wins: MIDI-FX repeats (arps) can emit later
        // note-ons for the same (track, transpose) — only the first audible
        // hit positions the recorded note.
        if let Some(target) = held
            .iter_mut()
            .flat_map(|note| note.targets.iter_mut())
            .find(|target| {
                !target.stamped
                    && target.track == stamp.track
                    && target.transpose.to_bits() == stamp.transpose.to_bits()
            })
        {
            if debug {
                eprintln!(
                    "[record-stamp] stamp track={} transpose={} beat={:.4} -> step={} phase={:.3} (press estimate was step={} phase={:.3})",
                    stamp.track,
                    stamp.transpose,
                    stamp.beat,
                    position.step,
                    position.phase,
                    target.position.step,
                    target.position.phase,
                );
            }
            target.position = position;
            target.stamped = true;
        } else if debug {
            eprintln!(
                "[record-stamp] stamp track={} transpose={} beat={:.4} UNMATCHED (no held unstamped target)",
                stamp.track, stamp.transpose, stamp.beat,
            );
        }
    });
}

/// `ESEQ_DEBUG_RECORD_STAMP=1` traces every live-recording stamp decision
/// (bead eseq-2awi): stamped positions as they drain, fallbacks at release,
/// and the final written step, so a note the performer hears misplaced can be
/// tied to the exact resolution path that produced it.
fn record_stamp_debug() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("ESEQ_DEBUG_RECORD_STAMP").is_ok_and(|value| value == "1"))
}

/// How long after its press a still-unstamped target may be restamped at the
/// render frontier on release: beyond a couple of audio blocks the trigger
/// has certainly been consumed, so a missing stamp means the note never went
/// through the audio callback at all (MIDI-FX-routed track) and sounded near
/// its PRESS — the frontier at release would be arbitrarily late.
const FRONTIER_FALLBACK_WINDOW: std::time::Duration = std::time::Duration::from_millis(35);

/// Member track the armed rack's pad for `note` plays, if any. This is the
/// whole of live pad routing (docs/drum-rack-v2-spec.md): the note selects a
/// pad by `pad_note`, the pad names a member track, and that track is
/// triggered directly — no `ByPitch` slot matching, and unmapped notes are
/// simply ignored.
fn armed_rack_pad_track(
    groups: &[sequencer::project::ProjectTrackGroup],
    armed_rack: Option<u64>,
    note: f32,
) -> Option<usize> {
    let group_id = armed_rack?;
    groups
        .iter()
        .find(|group| group.id == group_id)?
        .rack_pad_track(note.round() as i32)
}

/// Intercept keyboard events for live recording.
pub(crate) fn handle_recording_key(
    key: &crossterm::event::KeyEvent,
    app: &mut sequencer::app::App,
    state: &Arc<SequencerState>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    armed_rack: &Arc<Mutex<Option<u64>>>,
    recording: &Arc<AtomicBool>,
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    keyboard_octave: &Arc<std::sync::atomic::AtomicI32>,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    roll_record: &Arc<Mutex<RollRecordBuffer>>,
    ui_invalidations: &UiInvalidationQueue,
    sequence_roll_binding: bool,
) -> RecordingKeyOutcome {
    use crossterm::event::{KeyCode, KeyEventKind};

    let normalized_code = match key.code {
        KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
        code => code,
    };
    let roll_mode = state.transport.roll_mode.load(Ordering::Relaxed);
    let held_sequence_roll = held_notes
        .lock()
        .unwrap()
        .iter()
        .any(|note| note.sequence_roll_code == Some(normalized_code));
    if sequence_roll_binding || held_sequence_roll {
        // The momentary sequence-roll gesture is selected by the active mode's
        // named binding, not a hard-coded character. Release remains matched
        // to the held marker even if the mode or binding changes mid-hold.
        match key.kind {
            KeyEventKind::Press if roll_mode => {
                let mut held = held_notes.lock().unwrap();
                if !held
                    .iter()
                    .any(|note| note.sequence_roll_code == Some(normalized_code))
                {
                    held.push(HeldKeyboardNote {
                        key: '\0',
                        sequence_roll_code: Some(normalized_code),
                        transpose: 0.0,
                        press_time: Instant::now(),
                        targets: Vec::new(),
                    });
                    state
                        .transport
                        .sequence_rolling
                        .store(true, Ordering::Release);
                    eprintln!(
                        "[roll-debug] input SequenceRoll on=true key={:?} roll_mode={roll_mode}",
                        key.code,
                    );
                    state.push_roll_command(sequencer::sequencer::RollCommand::SequenceRoll {
                        on: true,
                    });
                }
                return RecordingKeyOutcome::Consumed;
            }
            KeyEventKind::Release => {
                let removed = {
                    let mut held = held_notes.lock().unwrap();
                    let pos = held
                        .iter()
                        .position(|note| note.sequence_roll_code == Some(normalized_code));
                    pos.map(|idx| held.remove(idx)).is_some()
                };
                if removed {
                    state
                        .transport
                        .sequence_rolling
                        .store(false, Ordering::Release);
                    eprintln!(
                        "[roll-debug] input SequenceRoll on=false key={:?}",
                        key.code,
                    );
                    state.push_roll_command(sequencer::sequencer::RollCommand::SequenceRoll {
                        on: false,
                    });
                    return RecordingKeyOutcome::Consumed;
                }
                return RecordingKeyOutcome::Ignored;
            }
            KeyEventKind::Press => {
                eprintln!(
                    "[roll-debug] input SequenceRoll ignored key={:?} roll_mode=false",
                    key.code,
                );
                return RecordingKeyOutcome::Ignored;
            }
            _ => return RecordingKeyOutcome::Consumed,
        }
    }

    let c = match normalized_code {
        KeyCode::Char(c) => c,
        _ => return RecordingKeyOutcome::Ignored,
    };

    // Roll mode (docs/rolling-core-spec.md 4.1): rate keys are consumed at
    // the live-keyboard seam, before the editor drops Release events.
    if roll_mode && key.modifiers.is_empty() {
        if let Some(rate) = sequencer::sequencer::Timebase::roll_rate_from_key(c) {
            if key.kind == KeyEventKind::Press {
                state
                    .transport
                    .roll_rate
                    .store(rate as u32, Ordering::Release);
                eprintln!(
                    "[roll-debug] input SetRate key={:?} rate={} grid_beats={}",
                    key.code,
                    rate.label(),
                    rate.step_beats(sequencer::sequencer::MAX_STEPS),
                );
                state.push_roll_command(sequencer::sequencer::RollCommand::SetRate { rate });
            }
            return RecordingKeyOutcome::Consumed;
        }
    }

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

            let octave = keyboard_octave.load(Ordering::Relaxed);
            let transpose = (note + octave) as f32;
            let press_time = Instant::now();
            let mut targets: Vec<LiveNoteTarget> = Vec::new();
            {
                let armed = record_armed.lock().unwrap();
                for (track, a) in armed.iter().enumerate() {
                    if *a {
                        targets.push(LiveNoteTarget {
                            track,
                            transpose,
                            position: press_record_position(state, track, press_time),
                            stamped: false,
                        });
                    }
                }
            }
            // Rack arm = pad-play mode: the key's note picks a pad, and the
            // pad's member track is triggered at base pitch. A note with no
            // pad is ignored; a member that is somehow armed as a track too
            // keeps its chromatic entry rather than sounding twice.
            if let Some(track) =
                armed_rack_pad_track(&app.groups, *armed_rack.lock().unwrap(), transpose)
            {
                if !targets.iter().any(|t| t.track == track) {
                    targets.push(LiveNoteTarget {
                        track,
                        transpose: 0.0,
                        position: press_record_position(state, track, press_time),
                        stamped: false,
                    });
                }
            }

            // With roll mode on the sequencer owns triggering outright:
            // presses never take the immediate live path — playing OR
            // stopped — they only arm the scheduler. A press while parked is
            // silent until Play, at which point it rolls from the first grid
            // line; this is also what makes press-then-Play (either order,
            // however racy) clean, with no out-of-time live hit leaking in
            // ahead of the quantized roll. (Amends rolling-core-spec 4.3's
            // "stopped behaves as normal live keys".)
            // Send note-on to audio thread for every target.
            for target in &targets {
                if roll_mode {
                    // No hit on keydown (F1): the scheduler fires the
                    // first hit at the next roll-grid boundary.
                    state.push_roll_command(sequencer::sequencer::RollCommand::NoteOn {
                        track: target.track,
                        transpose: target.transpose,
                    });
                } else {
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track: target.track,
                        transpose: target.transpose,
                        velocity: 1.0,
                        note_off: false,
                    });
                }
            }

            let triggered = !targets.is_empty();
            held.push(HeldKeyboardNote {
                key: c,
                sequence_roll_code: None,
                transpose,
                press_time,
                targets,
            });
            if triggered {
                RecordingKeyOutcome::Triggered
            } else {
                RecordingKeyOutcome::Consumed
            }
        }
        KeyEventKind::Release => {
            // Catch any note-on stamp that landed since the last frame drain
            // before the write below reads the target positions.
            apply_live_trigger_stamps(state, held_notes);
            // Find and remove the held note
            let held_entry = {
                let mut held = held_notes.lock().unwrap();
                let pos = held.iter().position(|note| note.key == c);
                pos.map(|idx| held.remove(idx))
            };

            // Record into pattern if recording + playing
            if let Some(mut note) = held_entry {
                // A tap so short its audio-thread stamp has not landed yet:
                // the trigger is still in flight and will sound at the render
                // frontier, so restamp there instead of keeping the
                // latency-subtracted press estimate. Older unstamped targets
                // (MIDI-FX-routed tracks) sounded near their press — the
                // frontier at release would be wrong for them, so they keep
                // the press estimate.
                if note.press_time.elapsed() < FRONTIER_FALLBACK_WINDOW
                    && note.targets.iter().any(|target| !target.stamped)
                {
                    if let Some(frontier) =
                        state.record_frontier_beats_at_instant(Instant::now())
                    {
                        for target in &mut note.targets {
                            if target.stamped {
                                continue;
                            }
                            if let Some(position) =
                                state.record_position_at_beat(target.track, frontier)
                            {
                                if record_stamp_debug() {
                                    eprintln!(
                                        "[record-stamp] frontier fallback track={} beat={:.4} -> step={} phase={:.3} (press estimate was step={} phase={:.3})",
                                        target.track,
                                        frontier,
                                        position.step,
                                        position.phase,
                                        target.position.step,
                                        target.position.phase,
                                    );
                                }
                                target.position = position;
                            }
                        }
                    }
                }
                let note = note;
                for target in &note.targets {
                    if roll_mode {
                        // Cancels every roll hit not yet inside the lookahead
                        // horizon (F3).
                        state.push_roll_command(sequencer::sequencer::RollCommand::NoteOff {
                            track: target.track,
                            transpose: target.transpose,
                        });
                    }
                    // The audio note-off always goes out so a sounding voice
                    // (rolled or normal) releases its envelope.
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track: target.track,
                        transpose: target.transpose,
                        velocity: 0.0,
                        note_off: true,
                    });
                }
                if roll_mode {
                    // Rolled hits were already written into live pattern
                    // state as they sounded (rolling-core-spec 6); the
                    // release schedules the deferred snapshot publish that
                    // makes them audible. The press-position write below
                    // would stamp a note where no hit sounded (F1).
                    if recording.load(Ordering::Relaxed) && state.is_playing() {
                        roll_record.lock().unwrap().note_released();
                    }
                    return RecordingKeyOutcome::Consumed;
                }

                if recording.load(Ordering::Relaxed) && state.is_playing() {
                    let bpm = state.transport.bpm.load(Ordering::Relaxed) as f64;
                    let secs_per_step = 60.0 / bpm / 4.0;
                    let hold_secs = note.press_time.elapsed().as_secs_f64();
                    let duration_steps = (hold_secs / secs_per_step).max(0.15).min(64.0) as f32;
                    let mut recorded_steps: Vec<(usize, usize)> = Vec::new();

                    let mut recorded_take = false;
                    let quantize = sequencer::record_quantize::RecordQuantize::from_atomic(
                        state.transport.record_quantize.load(Ordering::Relaxed) as u8,
                    );
                    // Recording engaged mid-playback: stamp the recording
                    // kind from the active view (unified-transport spec 5) —
                    // arrangement view promotes into arrangement capture so
                    // this performance records as a take; the session view
                    // stamps loop overdub into the looping live pattern.
                    if !note.targets.is_empty() {
                        app.stamp_recording_kind_for_note();
                    }
                    let song_authority = app.song_playback_authority_active();
                    let overdub = app.recording_kind
                        == Some(sequencer::app::song_transport::RecordingKind::Overdub);
                    for &LiveNoteTarget {
                        track,
                        transpose,
                        position,
                        stamped,
                    } in &note.targets
                    {
                        // Song-mode take recording (takes spec 8.4): while
                        // arrangement capture is active, an armed track's
                        // notes retarget into its pending take at
                        // clip-relative positions stamped on the
                        // latency-compensated record clock — the live
                        // pattern is NOT written.
                        if !overdub
                            && app.take_record_note(
                                track,
                                note.press_time,
                                transpose,
                                duration_steps,
                            )
                        {
                            recorded_take = true;
                            continue;
                        }
                        if song_authority {
                            if !overdub {
                                // The song owns playback for this lane: a
                                // note that could not be staged as a take
                                // (no record clock anchor yet) is dropped
                                // rather than folded — modulo the clip
                                // length — into the scene's looping pattern.
                                continue;
                            }
                            // Loop overdub claims the armed lane (spec 5.1):
                            // latch it so the target pattern is stable
                            // across row boundaries and the layered notes
                            // are audible. A lane currently playing a take
                            // refuses overdub — its note is dropped.
                            if !app.claim_overdub_lane(track) {
                                continue;
                            }
                        }
                        // Each target quantizes against its OWN track's
                        // timebase and length, which is what lets one rack
                        // performance land a 1/64 hat pad and a 1/16 kick pad
                        // on their own grids.
                        let num_steps = state.pattern.track_params[track].get_num_steps();
                        let (local_step, delay) = quantized_record_position(
                            position.step,
                            position.phase,
                            num_steps,
                            state.pattern.track_params[track].get_timebase(),
                            quantize,
                        );
                        if record_stamp_debug() {
                            eprintln!(
                                "[record-stamp] write track={} stamped={} step={} delay={:.3} (from step={} phase={:.3}, quantize={:?})",
                                track, stamped, local_step, delay, position.step, position.phase, quantize,
                            );
                        }
                        if !state.pattern.patterns[track].is_active(local_step) {
                            state.pattern.patterns[track].toggle_step(local_step);
                        }
                        state.pattern.chord_data[track].add_note_with_timing(
                            local_step,
                            transpose,
                            duration_steps,
                            delay,
                        );
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
                        if !recorded_steps.contains(&(track, local_step)) {
                            recorded_steps.push((track, local_step));
                        }
                    }
                    if !recorded_steps.is_empty() {
                        // Targeted invalidations instead of a ui_epoch bump
                        // (the live step-print contract): the epoch resync
                        // rebuilds every surface and refreshes the sequencer
                        // layout, which starved queued key events behind each
                        // recorded release — fast chord stabs sounded their
                        // second chord late on slow machines. These pushes
                        // update exactly the recorded trig's bindings on this
                        // frame's tick.
                        let mut published_tracks: Vec<usize> = Vec::new();
                        for &(track, step) in &recorded_steps {
                            ui_invalidations.push(UiInvalidation::StepBatch {
                                track,
                                steps: vec![step],
                            });
                            for param in [
                                StepParam::Transpose,
                                StepParam::Velocity,
                                StepParam::Duration,
                            ] {
                                ui_invalidations.push(UiInvalidation::Step {
                                    track,
                                    step,
                                    change: StepInvalidation::Param(param.into()),
                                });
                            }
                            ui_invalidations.push(UiInvalidation::Step {
                                track,
                                step,
                                change: StepInvalidation::DurationSpan,
                            });
                            if !published_tracks.contains(&track) {
                                published_tracks.push(track);
                            }
                        }
                        // Copy-on-write per-track publishes: a recorded note
                        // touches only its target tracks, so the scheduler
                        // hears it without a full-capture snapshot.
                        for track in published_tracks {
                            state.publish_scheduler_track(track);
                            ui_invalidations.push(UiInvalidation::PianoRoll {
                                track,
                                change: PianoRollInvalidation::Items,
                            });
                        }
                        return RecordingKeyOutcome::Recorded;
                    }
                    if recorded_take {
                        return RecordingKeyOutcome::RecordedTake;
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
        apply_live_trigger_stamps,
        armed_rack_pad_track, build_selection_value, current_step_param_number_picker_id,
        handle_metal_command_shortcut, handle_metal_soft_step_param_key,
        handle_number_picker_edit_key_for_widget,
        handle_recording_key, held_note_for_key,
        is_active_roll_rate_key, is_duplicate_shortcut_for, is_new_instrument_shortcut_for,
        normalize_command_shortcuts, normalize_command_shortcuts_for, note_from_key,
        number_picker_edit_state, quantized_record_position, sample_browser_search_shortcut_for,
        sequencer_history_shortcut, sequencer_tab_shortcut_index_for,
        should_route_to_live_keyboard,
        ExpandedStepProjectionRegistry, ExpandedStepViewport,
        HeldKeyboardNote, LiveNoteTarget, RecordingKeyOutcome, RollRecordBuffer,
        SequencerHistoryShortcut, SoftStepParamEdit, StepClipboardShortcut,
        UiInvalidationQueue, PROCESS_LANE_MODE_OFFSET, step_clipboard_shortcut_for,
    };
    #[cfg(not(target_os = "macos"))]
    use super::handle_arrangement_region_shortcut;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use eseqlisp::editor::ViewMode;
    use eseqlisp::ui::platform::{
        primary_shortcut_modifier, ShortcutPlatform, CURRENT_SHORTCUT_PLATFORM,
    };
    use eseqlisp::mode::BufferMode;
    use eseqlisp::vm::Value;
    use eseqlisp::widget_render::WidgetKeyEvent;
    use eseqlisp::HostCommand;
    use eseqlisp::{Editor, EditorConfig, Runtime};
    use sequencer::record_quantize::RecordQuantize;
    use sequencer::sequencer::{RecordPosition, SequencerState, StepParam, StepSnapshot, Timebase};
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn live_keyboard_routing_editor(live_keys: bool) -> Editor {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(do (define-mode \"routing-mode\" :live-keys {}) (set-buffer-mode \"routing-mode\"))",
                if live_keys { "true" } else { "false" },
            ))
            .expect("define routing mode");
        editor.refresh_runtime_side_effects();
        editor
    }

    fn soft_edit_test_app(state: Arc<SequencerState>) -> sequencer::app::App {
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = sequencer::app::App::new(
            state,
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            sequencer::app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = sequencer::sequencer::TrackRegistry::for_legacy_track_count(1)
            .expect("test track registry");
        app
    }

    #[test]
    fn global_sequencer_history_shortcuts_only_capture_ui_buffers() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let primary = if cfg!(target_os = "macos") {
            KeyModifiers::SUPER
        } else {
            KeyModifiers::CONTROL
        };

        assert_eq!(
            sequencer_history_shortcut(
                &editor,
                &KeyEvent::new(KeyCode::Char('z'), primary),
            ),
            None,
            "editable source buffers retain their own undo shortcut",
        );

        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        assert_eq!(
            sequencer_history_shortcut(
                &editor,
                &KeyEvent::new(KeyCode::Char('z'), primary),
            ),
            Some(SequencerHistoryShortcut::Undo),
        );
        assert_eq!(
            sequencer_history_shortcut(
                &editor,
                &KeyEvent::new(KeyCode::Char('Z'), primary | KeyModifiers::SHIFT),
            ),
            Some(SequencerHistoryShortcut::Redo),
        );
    }

    #[test]
    fn held_note_lookup_is_case_insensitive_for_release_matching() {
        let held = Arc::new(Mutex::new(vec![HeldKeyboardNote {
            key: 'a',
            sequence_roll_code: None,
            transpose: 0.0,
            press_time: Instant::now(),
            targets: vec![LiveNoteTarget {
                track: 0,
                transpose: 0.0,
                position: RecordPosition {
                    step: 0,
                    phase: 0.0,
                },
                stamped: false,
            }],
        }]));
        let key = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);

        assert!(held_note_for_key(&held, &key));
    }

    #[test]
    fn live_trigger_stamps_reposition_only_the_matching_unstamped_target() {
        let state = Arc::new(SequencerState::new(2, vec![]));
        let held = Arc::new(Mutex::new(vec![HeldKeyboardNote {
            key: 'a',
            sequence_roll_code: None,
            transpose: 5.0,
            press_time: Instant::now(),
            targets: vec![
                LiveNoteTarget {
                    track: 0,
                    transpose: 5.0,
                    position: RecordPosition { step: 9, phase: 0.9 },
                    stamped: false,
                },
                LiveNoteTarget {
                    track: 1,
                    transpose: 5.0,
                    position: RecordPosition { step: 9, phase: 0.9 },
                    stamped: false,
                },
            ],
        }]));

        // Beat 0.375 on a default 16-step sixteenth track: step 1, phase 0.5.
        state.push_live_trigger_stamp(0, 5.0, 0.375);
        // Wrong transpose: matches no target, must be dropped harmlessly.
        state.push_live_trigger_stamp(1, 7.0, 2.0);
        apply_live_trigger_stamps(&state, &held);
        {
            let held = held.lock().unwrap();
            let stamped = held[0].targets[0];
            assert!(stamped.stamped);
            assert_eq!(stamped.position.step, 1);
            assert!((stamped.position.phase - 0.5).abs() < 1e-4);
            let untouched = held[0].targets[1];
            assert!(!untouched.stamped);
            assert_eq!(untouched.position.step, 9);
        }

        // A later note-on for the same (track, transpose) — a MIDI-FX repeat —
        // must not move an already-stamped target.
        state.push_live_trigger_stamp(0, 5.0, 1.0);
        apply_live_trigger_stamps(&state, &held);
        assert_eq!(held.lock().unwrap()[0].targets[0].position.step, 1);
    }

    #[test]
    fn roll_rate_keys_are_active_without_any_armed_track() {
        let state = SequencerState::new(1, vec![]);
        let key = KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE);
        assert!(!is_active_roll_rate_key(&state, &key));

        state.transport.roll_mode.store(true, Ordering::Release);
        assert!(is_active_roll_rate_key(&state, &key));
        assert!(!is_active_roll_rate_key(
            &state,
            &KeyEvent::new(KeyCode::Char('7'), KeyModifiers::CONTROL),
        ));
    }

    #[test]
    fn live_keyboard_routing_requires_an_opted_in_major_mode() {
        let held = Arc::new(Mutex::new(Vec::new()));
        let note_key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        let roll_hold_key = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE);

        let source_editor = Editor::new(Runtime::new(), EditorConfig::default());
        assert!(!should_route_to_live_keyboard(
            &source_editor,
            &note_key,
            &held,
            false,
        ));
        assert!(!should_route_to_live_keyboard(
            &source_editor,
            &roll_hold_key,
            &held,
            true,
        ));

        let special_text_mode = live_keyboard_routing_editor(false);
        assert!(!should_route_to_live_keyboard(
            &special_text_mode,
            &note_key,
            &held,
            false,
        ));

        let sequencer_mode = live_keyboard_routing_editor(true);
        assert!(should_route_to_live_keyboard(
            &sequencer_mode,
            &note_key,
            &held,
            false,
        ));
        assert!(should_route_to_live_keyboard(
            &sequencer_mode,
            &roll_hold_key,
            &held,
            true,
        ));
    }

    /// A widget-only buffer has no text pane, so it gets live keys without
    /// declaring a `:live-keys` major mode. Requiring the opt-in muted every
    /// GUI buffer that never called `set-buffer-mode-for` (piano roll, sample
    /// browser, arrangement, transport).
    #[test]
    fn widget_only_buffers_accept_live_keys_without_a_mode_opt_in() {
        let held = Arc::new(Mutex::new(Vec::new()));
        let note_key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);

        let mut editor = live_keyboard_routing_editor(false);
        assert!(
            !should_route_to_live_keyboard(&editor, &note_key, &held, false),
            "a text-bearing buffer still needs its mode to opt in",
        );

        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        assert!(
            should_route_to_live_keyboard(&editor, &note_key, &held, false),
            "widget-only buffers route live keys without a mode opt-in",
        );

        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        assert!(
            !should_route_to_live_keyboard(&editor, &note_key, &held, false),
            "showing the text pane hands the keys back to the editor",
        );
    }

    #[test]
    fn focused_number_picker_edit_suppresses_live_and_roll_keys() {
        let mut editor = live_keyboard_routing_editor(true);
        editor.set_layout_viewport(30, 8);
        let tree = editor
            .runtime_mut()
            .eval_str(
                r#"(number-picker :key "routing-picker" :value 12 :min 0 :max 99
                     :decimals 0 :width 8 :height 1.4)"#,
            )
            .expect("build number picker")
            .expect("number picker widget tree");
        editor.active_buffer_mut().set_widget_tree(Some(tree.clone()), None);
        editor.runtime_mut().set_widget_tree(tree);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let _ = editor.widget_layout().expect("number picker layout");
        assert!(editor.focus_widget_by_stable_key("routing-picker", Some("number-picker")));

        editor.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        let picker_id = editor.focused_widget_id().expect("focused picker");
        assert!(number_picker_edit_state(picker_id).editing);

        let held = Arc::new(Mutex::new(Vec::new()));
        assert!(!should_route_to_live_keyboard(
            &editor,
            &KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE),
            &held,
            false,
        ));
        assert!(!should_route_to_live_keyboard(
            &editor,
            &KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE),
            &held,
            true,
        ));
    }

    #[test]
    fn held_note_release_bypasses_live_key_mode_gate() {
        let editor = Editor::new(Runtime::new(), EditorConfig::default());
        let held = Arc::new(Mutex::new(vec![HeldKeyboardNote {
            key: 'a',
            sequence_roll_code: None,
            transpose: 0.0,
            press_time: Instant::now(),
            targets: Vec::new(),
        }]));
        let mut release = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;

        assert!(should_route_to_live_keyboard(
            &editor,
            &release,
            &held,
            false,
        ));
    }

    #[test]
    fn semantic_sequence_roll_binding_handles_non_char_press_and_release() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        state.transport.roll_mode.store(true, Ordering::Release);
        let mut app = soft_edit_test_app(Arc::clone(&state));
        let record_armed = Arc::new(Mutex::new(vec![false]));
        let armed_rack: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let recording = Arc::new(AtomicBool::new(false));
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let keyboard_octave = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let held = Arc::new(Mutex::new(Vec::new()));
        let roll_record = Arc::new(Mutex::new(RollRecordBuffer::default()));
        let ui_invalidations = UiInvalidationQueue::new();
        let press = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);

        assert!(handle_recording_key(
            &press,
            &mut app,
            &state,
            &record_armed,
            &armed_rack,
            &recording,
            &keyboard_tx,
            &keyboard_octave,
            &held,
            &roll_record,
            &ui_invalidations,
            true,
        )
        .consumed());
        assert!(state.transport.sequence_rolling.load(Ordering::Acquire));
        assert!(matches!(
            state.drain_roll_commands().as_slice(),
            [sequencer::sequencer::RollCommand::SequenceRoll { on: true }]
        ));

        let mut release = press;
        release.kind = KeyEventKind::Release;
        assert!(handle_recording_key(
            &release,
            &mut app,
            &state,
            &record_armed,
            &armed_rack,
            &recording,
            &keyboard_tx,
            &keyboard_octave,
            &held,
            &roll_record,
            &ui_invalidations,
            false,
        )
        .consumed());
        assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
        assert!(matches!(
            state.drain_roll_commands().as_slice(),
            [sequencer::sequencer::RollCommand::SequenceRoll { on: false }]
        ));
    }

    /// Drum rack v2 slice 3 (`docs/drum-rack-v2-spec.md`, "Arming & live
    /// play"): a kit whose pads answer notes 36 and 38 on member tracks 1
    /// and 2.
    fn rack_fixture() -> sequencer::project::ProjectTrackGroup {
        sequencer::project::ProjectTrackGroup {
            id: 7,
            name: "Kit".to_string(),
            color: [0.5, 0.5, 0.5],
            collapsed: false,
            members: vec![1, 2],
            bus_id: 2,
            rack: Some(sequencer::project::ProjectRackConfig {
                pads: vec![
                    sequencer::project::ProjectRackPad {
                        pad_note: 36,
                        member: 0,
                    },
                    sequencer::project::ProjectRackPad {
                        pad_note: 38,
                        member: 1,
                    },
                ],
                choke_groups: vec![None, None],
            }),
            rack_members: Vec::new(),
        }
    }

    fn rack_test_app(state: Arc<SequencerState>, track_count: usize) -> sequencer::app::App {
        let mut app = soft_edit_test_app(state);
        app.tracks = (0..track_count).map(|i| format!("Track {i}")).collect();
        app.track_registry =
            sequencer::sequencer::TrackRegistry::for_legacy_track_count(track_count)
                .expect("test track registry");
        app.groups = vec![rack_fixture()];
        app
    }

    /// Press `key` with the octave placing `a` at MIDI 36, then release it.
    fn press_and_release_rack_key(
        key: char,
        app: &mut sequencer::app::App,
        state: &Arc<SequencerState>,
        record_armed: &Arc<Mutex<Vec<bool>>>,
        armed_rack: &Arc<Mutex<Option<u64>>>,
        recording: &Arc<AtomicBool>,
        keyboard_tx: &std::sync::mpsc::Sender<sequencer::sequencer::KeyboardTrigger>,
        held: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    ) -> RecordingKeyOutcome {
        let keyboard_octave = Arc::new(std::sync::atomic::AtomicI32::new(36));
        let roll_record = Arc::new(Mutex::new(RollRecordBuffer::default()));
        let ui_invalidations = UiInvalidationQueue::new();
        let mut event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE);
        event.kind = KeyEventKind::Press;
        handle_recording_key(
            &event,
            app,
            state,
            record_armed,
            armed_rack,
            recording,
            keyboard_tx,
            &keyboard_octave,
            held,
            &roll_record,
            &ui_invalidations,
            false,
        );
        event.kind = KeyEventKind::Release;
        handle_recording_key(
            &event,
            app,
            state,
            record_armed,
            armed_rack,
            recording,
            keyboard_tx,
            &keyboard_octave,
            held,
            &roll_record,
            &ui_invalidations,
            false,
        )
    }

    #[test]
    fn armed_rack_maps_pad_notes_to_member_tracks_and_ignores_the_rest() {
        let groups = [rack_fixture()];
        assert_eq!(
            armed_rack_pad_track(&groups, Some(7), 36.0),
            Some(1),
            "pad note 36 plays the first member track"
        );
        assert_eq!(armed_rack_pad_track(&groups, Some(7), 38.0), Some(2));
        assert_eq!(
            armed_rack_pad_track(&groups, Some(7), 37.0),
            None,
            "a note with no pad is ignored"
        );
        assert_eq!(
            armed_rack_pad_track(&groups, None, 36.0),
            None,
            "no rack armed means no pad routing at all"
        );
        assert_eq!(
            armed_rack_pad_track(&groups, Some(99), 36.0),
            None,
            "an arm pointing at no live group routes nothing"
        );
    }

    #[test]
    fn rack_arm_plays_pad_member_track_at_base_pitch() {
        let state = Arc::new(SequencerState::new(3, vec![]));
        let mut app = rack_test_app(Arc::clone(&state), 3);
        let record_armed = Arc::new(Mutex::new(vec![false; 3]));
        let armed_rack = Arc::new(Mutex::new(Some(7)));
        let recording = Arc::new(AtomicBool::new(false));
        let (keyboard_tx, keyboard_rx) = std::sync::mpsc::channel();
        let held = Arc::new(Mutex::new(Vec::new()));

        // 'a' at this octave is MIDI 36 — the first pad.
        press_and_release_rack_key(
            'a',
            &mut app,
            &state,
            &record_armed,
            &armed_rack,
            &recording,
            &keyboard_tx,
            &held,
        );
        let on = keyboard_rx.recv().expect("pad note-on");
        assert_eq!(on.track, 1, "pad 36 triggers its member track");
        assert_eq!(
            on.transpose, 0.0,
            "a pad plays its member at base pitch, not at the key's pitch"
        );
        assert!(!on.note_off);
        let off = keyboard_rx.recv().expect("pad note-off");
        assert_eq!((off.track, off.transpose, off.note_off), (1, 0.0, true));

        // 'w' is MIDI 37 — no pad answers it.
        press_and_release_rack_key(
            'w',
            &mut app,
            &state,
            &record_armed,
            &armed_rack,
            &recording,
            &keyboard_tx,
            &held,
        );
        assert!(
            keyboard_rx.try_recv().is_err(),
            "an unmapped note triggers nothing"
        );
    }

    #[test]
    fn arming_a_member_track_plays_it_chromatically_instead_of_as_a_pad() {
        let state = Arc::new(SequencerState::new(3, vec![]));
        let mut app = rack_test_app(Arc::clone(&state), 3);
        // Member track 1 armed as an ordinary track; the rack is not armed —
        // the two are mutually exclusive.
        let record_armed = Arc::new(Mutex::new(vec![false, true, false]));
        let armed_rack = Arc::new(Mutex::new(None));
        let recording = Arc::new(AtomicBool::new(false));
        let (keyboard_tx, keyboard_rx) = std::sync::mpsc::channel();
        let held = Arc::new(Mutex::new(Vec::new()));

        press_and_release_rack_key(
            'a',
            &mut app,
            &state,
            &record_armed,
            &armed_rack,
            &recording,
            &keyboard_tx,
            &held,
        );
        let on = keyboard_rx.recv().expect("chromatic note-on");
        assert_eq!(
            (on.track, on.transpose),
            (1, 36.0),
            "an armed member plays the key's pitch, not a pad at base pitch"
        );
        let off = keyboard_rx.recv().expect("chromatic note-off");
        assert_eq!((off.track, off.transpose, off.note_off), (1, 36.0, true));
        assert!(
            keyboard_rx.try_recv().is_err(),
            "only the armed track sounds"
        );
    }

    #[test]
    fn rack_recording_quantizes_each_pad_against_its_own_member_timebase() {
        let state = Arc::new(SequencerState::new(3, vec![]));
        // Kick pad: 16 steps of 1/16. Hat pad: 64 steps of 1/64. Both are
        // passing step 5 when the pads are hit, so only the member's own
        // timebase decides where the hit lands.
        state.pattern.track_params[1].set_num_steps(16);
        state.pattern.track_params[1].set_timebase(Timebase::Sixteenth);
        state.pattern.track_params[2].set_num_steps(64);
        state.pattern.track_params[2].set_timebase(Timebase::SixtyFourth);
        for track in [1usize, 2] {
            state.transport.track_playheads[track].store(5, Ordering::Relaxed);
            state.transport.track_playhead_phases[track].store(0f32.to_bits(), Ordering::Relaxed);
        }
        state.transport.playing.store(true, Ordering::Relaxed);
        state
            .transport
            .record_quantize
            .store(RecordQuantize::Quarter as u32, Ordering::Relaxed);

        let mut app = rack_test_app(Arc::clone(&state), 3);
        let record_armed = Arc::new(Mutex::new(vec![false; 3]));
        let armed_rack = Arc::new(Mutex::new(Some(7)));
        let recording = Arc::new(AtomicBool::new(true));
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let held = Arc::new(Mutex::new(Vec::new()));

        for key in ['a', 's'] {
            assert_eq!(
                press_and_release_rack_key(
                    key,
                    &mut app,
                    &state,
                    &record_armed,
                    &armed_rack,
                    &recording,
                    &keyboard_tx,
                    &held,
                ),
                RecordingKeyOutcome::Recorded,
                "pad {key} should record into its member pattern"
            );
        }

        // 1/16 track: a quarter-note grid is 4 steps, so step 5 snaps to 4.
        assert!(state.pattern.patterns[1].is_active(4));
        assert_eq!(state.pattern.chord_data[1].get(4, 0), 0.0);
        // 1/64 track: the same grid is 16 steps, so step 5 snaps to 0 — a
        // different landing step from the same performance.
        assert!(state.pattern.patterns[2].is_active(0));
        assert_eq!(state.pattern.chord_data[2].get(0, 0), 0.0);
        assert!(
            !state.pattern.patterns[2].is_active(4),
            "the hat pad must not borrow the kick's grid"
        );
        assert!(
            (0..16).all(|step| !state.pattern.patterns[0].is_active(step)),
            "a track outside the rack records nothing"
        );
    }

    #[test]
    fn live_note_map_uses_lowercase_keys() {
        assert_eq!(note_from_key('a'), Some(0));
        assert_eq!(note_from_key('A'), None);
    }

    #[test]
    fn record_quantize_off_preserves_phase_as_per_note_delay() {
        assert_eq!(
            quantized_record_position(3, 0.37, 16, Timebase::Sixteenth, RecordQuantize::Off),
            (3, 0.37)
        );
    }

    #[test]
    fn sixteenth_record_quantize_rounds_to_nearest_step() {
        assert_eq!(
            quantized_record_position(3, 0.49, 16, Timebase::Sixteenth, RecordQuantize::Sixteenth),
            (3, 0.0)
        );
        assert_eq!(
            quantized_record_position(3, 0.50, 16, Timebase::Sixteenth, RecordQuantize::Sixteenth),
            (4, 0.0)
        );
    }

    #[test]
    fn coarse_record_quantize_snaps_and_wraps_the_pattern() {
        assert_eq!(
            quantized_record_position(5, 0.7, 16, Timebase::Sixteenth, RecordQuantize::Quarter),
            (4, 0.0)
        );
        assert_eq!(
            quantized_record_position(15, 0.9, 16, Timebase::Sixteenth, RecordQuantize::Quarter),
            (0, 0.0)
        );
    }

    #[test]
    fn up_down_switch_tracks_outside_piano_roll() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str("(defstate eseq.seq-core-state/selected-bus -1)")
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
    fn up_down_uses_lisp_visual_track_order_for_non_contiguous_group_members() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-core-state/selected-bus -1)
                (def eseq.drum-rack-v2/track-relative (track delta)
                  (if (= track 7)
                    (if (> delta 0) 10 1)
                    (if (= track 10)
                      (if (< delta 0) 7 11)
                      nil)))
                "#,
            )
            .expect("install visual-order navigation fixture");
        let current_track = Arc::new(AtomicUsize::new(7));
        let native_track = Arc::clone(&current_track);
        editor
            .runtime_mut()
            .register_native("seq-set-track", move |args, _ctx| {
                let Some(Value::Number(track)) = args.first() else {
                    return Err("expected track".to_string());
                };
                native_track.store(*track as usize, Ordering::Relaxed);
                Ok(Value::Number(*track))
            });
        let state = Arc::new(SequencerState::new(12, vec![]));
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
        assert_eq!(current_track.load(Ordering::Relaxed), 10);

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(current_track.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn open_modal_blocks_global_track_navigation_shortcuts() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let tree = editor
            .runtime_mut()
            .eval_str(
                r#"
                (modal :is-open true
                  (button "inside" :focusable true))
                "#,
            )
            .expect("build modal")
            .expect("modal widget tree");
        editor
            .active_buffer_mut()
            .set_widget_tree(Some(tree.clone()), None);
        editor.runtime_mut().set_widget_tree(tree);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.set_layout_viewport(40, 20);
        assert!(editor.modal_is_open());

        let current_track = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(SequencerState::new(3, vec![]));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(
            !handle_metal_command_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "the app shortcut layer must yield to the open modal"
        );
        assert_eq!(current_track.load(Ordering::Relaxed), 0);
    }

    fn sample_browser_keyboard_editor() -> Editor {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.set_layout_viewport(100, 30);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate sbrowser-tab "samples")
                (defstate eseq.browser/search-filter "")
                (defstate sbrowser-modified-label "")
                (defstate main-view-toggle-count 0)
                (def eseq.seq-panels/seq-toggle-arrangement ()
                  (set! main-view-toggle-count (+ main-view-toggle-count 1)))
                (def eseq.browser/active-tree-key ()
                  (if (= sbrowser-tab "samples") "samples-tab-tree" "instruments-tab-tree"))
                (def eseq.browser/next-tab ()
                  (set! sbrowser-tab
                    (if (= sbrowser-tab "samples") "instruments" "samples")))
                (def eseq.browser/sample-browser-here () (switch-to-buffer "*samples*"))
                (def sbrowser-keyboard-items ()
                  (list (dict :label "kick.wav" :path "samples/kick.wav")))
                (def sbrowser-keyboard-panel ()
                  (v-stack :width :fill :height :fill
                    (text-input
                      :key "sbrowser-search-input"
                      :width :fill
                      :value eseq.browser/search-filter
                      :placeholder "Search"
                      :on-change (lambda (v) (set! eseq.browser/search-filter v)))
                    (if (= sbrowser-tab "samples")
                      (tree
                        :key "samples-tab-tree"
                        :width :fill
                        :focusable true
                        :items (sbrowser-keyboard-items)
                        :on-modified-activate
                          (lambda (item) (set! sbrowser-modified-label (get item :label))))
                      (tree
                        :key "instruments-tab-tree"
                        :width :fill
                        :focusable true
                        :items (sbrowser-keyboard-items)))))
                (effect-buffer "*samples*" (sbrowser-keyboard-panel))
                "#,
            )
            .expect("install sample browser keyboard fixture");
        editor.refresh_runtime_side_effects();
        editor.open_scratch_buffer("*sequencer*", "");
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
    }

    fn empty_command_state() -> (
        Arc<SequencerState>,
        Arc<AtomicUsize>,
        Arc<Mutex<HashSet<usize>>>,
        Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>>,
    ) {
        (
            Arc::new(SequencerState::new(1, vec![])),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Mutex::new(HashSet::new())),
            Arc::new(Mutex::new(None)),
        )
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_step_copy_and_paste_require_shifted_control() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str("(def eseq.seq-core-state/current-step () 4)")
            .expect("install cursor fixture");
        let state = Arc::new(SequencerState::new(1, vec![]));
        state.pattern.patterns[0].set_step_active(1, true);
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::from([1])));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        let clipboard_modifiers = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('c'), clipboard_modifiers),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(step_clipboard.lock().unwrap().is_some());

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('v'), clipboard_modifiers),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(editor.drain_host_commands().iter().any(|command| matches!(
            command,
            HostCommand::Custom { name, .. } if name == "paste-steps"
        )));
    }

    #[test]
    fn step_copy_uses_the_platform_clipboard_modifier() {
        for (platform, accepted, rejected) in [
            (
                ShortcutPlatform::MacOS,
                KeyModifiers::SUPER,
                KeyModifiers::CONTROL,
            ),
            (
                ShortcutPlatform::Other,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                KeyModifiers::CONTROL,
            ),
        ] {
            assert_eq!(
                step_clipboard_shortcut_for(
                    &KeyEvent::new(KeyCode::Char('c'), rejected),
                    platform,
                ),
                None,
            );
            assert_eq!(
                step_clipboard_shortcut_for(
                    &KeyEvent::new(KeyCode::Char('c'), accepted),
                    platform,
                ),
                Some(StepClipboardShortcut::Copy),
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_arrangement_clipboard_requires_shift_but_duplicate_does_not() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*arrangement*", "");
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str("(def SEQ (dict :song-region '(0 1) :song-bound-clip nil))")
            .expect("install arrangement region fixture");

        for key in ['c', 'v'] {
            assert!(!handle_arrangement_region_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL),
            ));
        }

        let clipboard_modifiers = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        for (key, expected) in [('c', "song-region-copy"), ('v', "song-region-paste")] {
            assert!(handle_arrangement_region_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Char(key), clipboard_modifiers),
            ));
            assert!(matches!(
                editor.drain_host_commands().as_slice(),
                [HostCommand::Custom { name, .. }] if name == expected
            ));
        }

        assert!(handle_arrangement_region_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        ));
        assert!(matches!(
            editor.drain_host_commands().as_slice(),
            [HostCommand::Custom { name, .. }] if name == "song-region-duplicate"
        ));
    }

    #[test]
    fn platform_primary_shortcut_shapes_cover_tabs_browser_focus_and_duplicate() {
        for (platform, primary, non_primary) in [
            (ShortcutPlatform::MacOS, KeyModifiers::SUPER, KeyModifiers::CONTROL),
            (ShortcutPlatform::Other, KeyModifiers::CONTROL, KeyModifiers::SUPER),
        ] {
            let tab = KeyEvent::new(KeyCode::Char('3'), primary);
            assert_eq!(sequencer_tab_shortcut_index_for(&tab, platform), Some(3));
            assert!(sample_browser_search_shortcut_for(
                &KeyEvent::new(KeyCode::Char('f'), primary),
                platform,
            ));
            assert!(is_duplicate_shortcut_for(
                &KeyEvent::new(KeyCode::Char('d'), primary),
                platform,
            ));
            assert!(is_new_instrument_shortcut_for(
                &KeyEvent::new(KeyCode::Char('i'), primary),
                platform,
            ));

            assert_eq!(
                sequencer_tab_shortcut_index_for(
                    &KeyEvent::new(KeyCode::Char('3'), non_primary),
                    platform,
                ),
                None,
            );
            assert!(!sample_browser_search_shortcut_for(
                &KeyEvent::new(KeyCode::Char('f'), non_primary),
                platform,
            ));
            assert!(!is_duplicate_shortcut_for(
                &KeyEvent::new(KeyCode::Char('d'), non_primary),
                platform,
            ));
        }
    }

    #[test]
    fn command_shortcut_normalization_preserves_raw_clipboard_events_and_linux_control_a() {
        for key in ['c', 'v'] {
            let event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::SUPER);
            assert_eq!(normalize_command_shortcuts(event), event);
        }

        let linux_select_all = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(
            normalize_command_shortcuts_for(linux_select_all, ShortcutPlatform::Other),
            linux_select_all,
        );
        assert_eq!(
            normalize_command_shortcuts_for(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER),
                ShortcutPlatform::MacOS,
            ),
            linux_select_all,
        );
    }

    #[test]
    fn platform_primary_r_arms_current_track_exclusively_only_outside_text_editing() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*source*", "");
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def armed-with (state -1))
                (def seq-arm-track-exclusive (track)
                  (set! armed-with track))
                "#,
            )
            .expect("install exclusive-arm hook");
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();
        current_track.store(3, Ordering::Relaxed);

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('r'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("armed-with").unwrap(),
            Some(Value::Number(3.0)),
            "the shortcut must arm the current track"
        );

        assert!(
            !handle_metal_command_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "plain r stays a live-keyboard note"
        );

        // Text editing keeps the platform chord (Ctrl+R search-backward on
        // Linux).
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('r'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
    }

    #[test]
    fn platform_primary_b_focuses_the_transport_bpm_picker() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*transport*", "");
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (effect
                  (v-stack :width 30 :height 4
                    (number-picker :key "transport-bpm" :value 120
                      :min 20 :max 300 :width 7 :height 1.2)))
                "#,
            )
            .expect("build transport bpm fixture");
        editor.refresh_runtime_side_effects();
        editor.set_layout_viewport(40, 10);
        editor.refresh_runtime_side_effects();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('b'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        let focused = editor
            .focused_widget_node()
            .expect("bpm picker must be focused");
        assert_eq!(focused.widget_type, "number-picker");
        assert_eq!(focused.stable_key.as_deref(), Some("transport-bpm"));

        // In a text-editing buffer the chord is left alone (Ctrl+B move-left
        // and the custom-UI reload piggybacked on it).
        editor.open_scratch_buffer("*source*", "");
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('b'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
    }

    #[test]
    fn platform_primary_comma_toggles_trigger_recording_without_claiming_plain_comma() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*source*", "");
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def record-toggle-count (state 0))
                (def seq-toggle-record ()
                  (set! record-toggle-count (+ record-toggle-count 1)))
                "#,
            )
            .expect("install record toggle hook");
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char(','), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("record-toggle-count")
                .unwrap(),
            Some(Value::Number(1.0))
        );

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char(','), KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("record-toggle-count")
                .unwrap(),
            Some(Value::Number(1.0))
        );
    }

    #[test]
    fn slash_does_not_focus_sample_browser_search_from_ui_buffers() {
        let mut editor = sample_browser_keyboard_editor();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert_eq!(editor.active_buffer().name, "*sequencer*");
        assert!(editor.focused_widget_node().is_none());
    }

    #[test]
    fn platform_primary_f_focuses_sample_browser_search_from_ui_buffers() {
        let mut editor = sample_browser_keyboard_editor();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('f'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert_eq!(editor.active_buffer().name, "*samples*");
        let focused = editor
            .focused_widget_node()
            .expect("Cmd+F should focus the browser search");
        assert_eq!(focused.widget_type, "text-input");
        assert_eq!(focused.stable_key.as_deref(), Some("sbrowser-search-input"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_control_f_remains_available_to_text_mode() {
        let mut editor = sample_browser_keyboard_editor();
        editor.open_scratch_buffer("*source*", "line one\nline two\n");
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(editor.active_buffer().name, "*source*");
        assert!(editor.focused_widget_node().is_none());
    }

    #[test]
    fn selected_plock_row_blocks_global_selected_step_delete_shortcut() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def delete-count (state 0))
                (def plock-row-selected (state false))
                (def eseq.step-grid-interactions/delete-selected-steps ()
                  (set! delete-count (+ delete-count 1)))
                (def eseq.effects.track-panels/plock-row-selected? () plock-row-selected)
                "#,
            )
            .expect("install step/plock delete hooks");

        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();
        selected_steps.lock().unwrap().insert(3);

        editor
            .runtime_mut()
            .eval_str("(set! plock-row-selected true)")
            .expect("select plock row");
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("delete-count").unwrap(),
            Some(Value::Number(0.0)),
            "global step deletion must not run while a plock row is selected"
        );

        editor
            .runtime_mut()
            .eval_str("(set! plock-row-selected false)")
            .expect("clear plock row selection");
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("delete-count").unwrap(),
            Some(Value::Number(1.0)),
            "global step deletion should still work once no plock row is selected"
        );
    }

    #[test]
    fn slash_does_not_steal_focused_search_or_text_only_buffers() {
        let mut editor = sample_browser_keyboard_editor();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('f'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(editor.active_buffer().name, "*samples*");

        editor.open_scratch_buffer("*source*", "");
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(editor.active_buffer().name, "*source*");
    }

    #[test]
    fn sample_browser_search_up_down_focuses_active_tree() {
        let mut editor = sample_browser_keyboard_editor();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('f'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        let focused = editor
            .focused_widget_node()
            .expect("down from search should focus the active tree");
        assert_eq!(focused.widget_type, "tree");
        assert_eq!(focused.stable_key.as_deref(), Some("samples-tab-tree"));
    }

    #[test]
    fn sample_browser_tree_does_not_override_global_tab_view_toggle() {
        let mut editor = sample_browser_keyboard_editor();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('f'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-tab").unwrap(),
            Some(Value::String("samples".to_string())),
            "plain Tab should no longer cycle browser-local tabs"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("main-view-toggle-count")
                .unwrap(),
            Some(Value::Number(1.0)),
            "plain Tab should route to the app-level session/arrangement toggle"
        );
    }

    #[test]
    fn modified_enter_on_sample_browser_tree_uses_modified_activate() {
        let mut editor = sample_browser_keyboard_editor();
        let samples_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("samples buffer")
            .id;
        editor.set_active_buffer(samples_id);
        editor.refresh_runtime_side_effects();
        assert!(editor.focus_widget_by_stable_key("samples-tab-tree", Some("tree")));

        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-modified-label")
                .unwrap(),
            Some(Value::String("kick.wav".to_string()))
        );
    }

    #[test]
    fn plain_arrows_navigate_from_piano_roll_ui_buffer() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*piano-roll*", "");
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let current_track = Arc::new(AtomicUsize::new(0));
        let native_track = Arc::clone(&current_track);
        editor
            .runtime_mut()
            .register_native("seq-set-track", move |args, _ctx| {
                let Some(Value::Number(track)) = args.first() else {
                    return Err("expected track".to_string());
                };
                native_track.store(*track as usize, Ordering::Relaxed);
                Ok(Value::Number(*track))
            });
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-core-state/selected-bus 1)
                (defstate cursor-left-count 0)
                (defstate cursor-right-count 0)
                (defstate cursor-select-left-count 0)
                (defstate cursor-select-right-count 0)
                (defstate cursor-toggle-count 0)
                (defstate selected-track-via-seqv -1)
                (def eseq.step-grid-interactions/cursor-left () (set! cursor-left-count (+ cursor-left-count 1)))
                (def eseq.step-grid-interactions/cursor-right () (set! cursor-right-count (+ cursor-right-count 1)))
                (def eseq.step-grid-interactions/cursor-select-left () (set! cursor-select-left-count (+ cursor-select-left-count 1)))
                (def eseq.step-grid-interactions/cursor-select-right () (set! cursor-select-right-count (+ cursor-select-right-count 1)))
                (def eseq.step-grid-interactions/cursor-toggle () (set! cursor-toggle-count (+ cursor-toggle-count 1)))
                (def eseq.sequencer/select-track-for-edit (track)
                  (do
                    (set! eseq.seq-core-state/selected-bus -1)
                    (set! selected-track-via-seqv track)
                    (seq-set-track track)))
                "#,
            )
            .expect("install arrow navigation handlers");
        let state = Arc::new(SequencerState::new(3, vec![]));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("cursor-left-count").unwrap(),
            Some(Value::Number(1.0))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("cursor-right-count").unwrap(),
            Some(Value::Number(1.0))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("cursor-select-left-count")
                .unwrap(),
            Some(Value::Number(1.0))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("cursor-select-right-count")
                .unwrap(),
            Some(Value::Number(1.0))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("cursor-toggle-count")
                .unwrap(),
            Some(Value::Number(1.0))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(current_track.load(Ordering::Relaxed), 1);
        assert_eq!(
            editor.runtime_mut().eval_str("eseq.seq-core-state/selected-bus").unwrap(),
            Some(Value::Number(-1.0))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("selected-track-via-seqv")
                .unwrap(),
            Some(Value::Number(1.0))
        );
    }

    #[test]
    fn plain_arrows_do_not_navigate_while_widget_is_focused() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.set_layout_viewport(80, 20);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate cursor-left-count 0)
                (defstate cursor-toggle-count 0)
                (def eseq.step-grid-interactions/cursor-left () (set! cursor-left-count (+ cursor-left-count 1)))
                (def eseq.step-grid-interactions/cursor-toggle () (set! cursor-toggle-count (+ cursor-toggle-count 1)))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :tool :draw
                    :lanes (list (dict :id 0 :label "L0"))
                    :items ()
                    :view-start 0
                    :view-duration 16
                    :on-action |e| e))
                "#,
            )
            .expect("install focused widget fixture");
        editor.handle_mouse_precise(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
            0,
            0,
            80,
            20,
            10.0,
            3.0,
        );
        assert!(
            editor.focused_widget_id().is_some(),
            "fixture should focus a widget before testing global arrow routing"
        );
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

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
            Some(Value::Number(0.0))
        );
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("cursor-toggle-count")
                .unwrap(),
            Some(Value::Number(0.0))
        );
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
                (def eseq.seq-panels/seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
                (def eseq.seq-panels/seq-toggle-arrangement () (set! tab-target "arrangement"))
                (def eseq.seq-panels/seq-toggle-piano-roll-placement () (set! tab-target "placement"))
                (def eseq.seq-panels/seq-toggle-mixer-panel () (set! tab-target "mixer"))
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
            Some(eseqlisp::vm::Value::String("arrangement".to_string()))
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
            Some(eseqlisp::vm::Value::String("piano".to_string()))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
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
    fn plain_tab_and_character_bindings_retain_editor_behavior_in_vim_insert_mode() {
        let mut editor = Editor::new(
            Runtime::new(),
            EditorConfig {
                vim_mode: true,
                ..EditorConfig::default()
            },
        );
        editor.open_scratch_buffer("*scratch*", "(if test\n:4t)");
        editor.active_buffer_mut().cursor = (1, 0);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate tab-target "")
                (def eseq.seq-panels/seq-toggle-arrangement ()
                  (set! tab-target "arrangement"))
                (bind-key "Tab" "eseq.seq-panels/seq-toggle-arrangement")
                (bind-key ";" "eseq.seq-panels/seq-toggle-arrangement")
                "#,
            )
            .expect("install application Tab binding");
        editor.refresh_runtime_side_effects();
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(
            editor.active_vim_input_mode(),
            Some(eseqlisp::editor::VimInputMode::Insert)
        );
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &tab,
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        editor.handle_key(tab);
        editor.handle_key(KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE));

        assert_eq!(editor.active_buffer().text(), "(if test\n  ;:4t)");
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(Value::String(String::new())),
            "the application Tab command must not run while Vim is inserting text"
        );

        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            editor.active_vim_input_mode(),
            Some(eseqlisp::editor::VimInputMode::Normal)
        );
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &tab,
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(Value::String("arrangement".to_string())),
            "the application Tab command should run in Vim normal mode"
        );
    }

    #[test]
    fn platform_primary_number_selects_visible_sequencer_step_tab() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let sequencer_id =
            editor.open_scratch_buffer_with_mode("*sequencer*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let tab_id = editor.open_scratch_buffer_with_mode("*script-tab*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let samples_id =
            editor.open_scratch_buffer_with_mode("*samples*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-step-tabs/step-panel-buffer "*sequencer*")
                (defstate eseq.seq-step-tabs/remembered-step-panel-buffer "*sequencer*")
                (defstate selected-step-tab "")
                (def eseq.seq-panels/seq-current-step-buffer () eseq.seq-step-tabs/step-panel-buffer)
                (def eseq.seq-step-tabs/seq-select-main-step-tab-by-index (index)
                  (if (= index 2)
                    (do
                      (set! selected-step-tab "script")
                      (set! eseq.seq-step-tabs/step-panel-buffer "*script-tab*")
                      (set! eseq.seq-step-tabs/remembered-step-panel-buffer "*script-tab*")
                      (set-window-buffer "*script-tab*")
                      true)
                    false))
                (set-layout
                  (list :cols
                    0.5 (list :buf "*sequencer*"
                          :tabs (list (list "Seq" "*sequencer*") (list "Script" "*script-tab*"))
                          :hide-status true)
                    0.5 (list :buf "*samples*" :hide-status true)))
                "#,
            )
            .expect("install sequencer tab fixture");
        editor.refresh_runtime_side_effects();
        assert!(
            editor.switch_active_tile_to_buffer_named("*samples*"),
            "samples tile should be visible"
        );
        assert_eq!(editor.active_buffer().id, samples_id);

        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('2'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert_eq!(editor.active_buffer().id, tab_id);
        assert_eq!(
            editor.runtime_mut().eval_str("selected-step-tab").unwrap(),
            Some(eseqlisp::vm::Value::String("script".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("eseq.seq-step-tabs/step-panel-buffer").unwrap(),
            Some(eseqlisp::vm::Value::String("*script-tab*".to_string()))
        );
        assert_ne!(
            editor.active_buffer().id,
            sequencer_id,
            "the shortcut should switch to the requested tab, not just focus the base sequencer buffer"
        );
    }

    #[test]
    fn plain_tab_toggles_arrangement_even_with_focused_sequencer_widget() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let transport_id =
            editor.open_scratch_buffer_with_mode("*transport*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let sequencer_id =
            editor.open_scratch_buffer_with_mode("*sequencer*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.set_active_buffer(sequencer_id);
        assert_eq!(editor.active_buffer().name, "*sequencer*");
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate tab-target "")
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :tool :draw
                    :lanes (list (dict :id 0 :label "L0"))
                    :items ()
                    :view-start 0
                    :view-duration 16
                    :on-action |e| e))
                (def eseq.seq-panels/seq-toggle-arrangement ()
                  (set! tab-target "arrangement"))
                (def eseq.seq-panels/seq-toggle-mixer-panel ()
                  (set! tab-target "mixer"))
                "#,
            )
            .expect("install tab handler");
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (set-layout
                  (list :cols
                    0.5 (list :buf "*transport*" :hide-status true)
                    0.5 (list :buf "*sequencer*" :hide-status true)))
                "#,
            )
            .expect("install visible sequencer layout");
        editor.refresh_runtime_side_effects();
        assert!(
            editor.switch_active_tile_to_buffer_named("*transport*"),
            "transport tile should be visible"
        );
        assert_eq!(
            editor.active_buffer().id,
            transport_id,
            "fixture should start on the first layout tile"
        );
        assert!(
            editor.switch_active_tile_to_buffer_named("*sequencer*"),
            "sequencer tile should be visible"
        );
        editor.handle_mouse_precise(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
            0,
            0,
            30,
            8,
            10.0,
            3.0,
        );
        assert!(
            editor.focused_widget_id().is_some(),
            "fixture should cover the focused-widget path that used to bypass the Tab fast path"
        );
        editor
            .runtime_mut()
            .eval_str(r#"(set! tab-target "")"#)
            .expect("reset tab target");
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
            Some(eseqlisp::vm::Value::String("arrangement".to_string())),
            "plain Tab should toggle the app view even when a sequencer widget has focus"
        );
    }

    #[test]
    fn plain_tab_stays_in_patch_editor_with_focused_patcher() {
        let path = std::env::temp_dir().join(format!(
            "eseq-focused-patcher-tab-{}.lisp",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "(def sig (in 1))\n(out sig 1)\n").unwrap();

        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.set_layout_viewport(80, 20);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(&format!(
                r#"
                (defstate tab-target "")
                (def eseq.seq-panels/seq-toggle-arrangement () (set! tab-target "arrangement"))
                (effect
                  (patcher
                    :height 10
                    :intent :effect
                    :path "{}"))
                "#,
                path.display()
            ))
            .expect("install focused patcher fixture");
        editor.set_layout_viewport(80, 20);
        editor.handle_mouse_precise(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 20,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            0,
            0,
            80,
            20,
            20.0,
            5.0,
        );
        assert_eq!(
            editor
                .focused_widget_node()
                .expect("focused widget")
                .widget_type,
            "patcher"
        );

        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        assert!(
            handle_metal_command_shortcut(
                &mut editor,
                &tab,
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "plain Tab should be consumed (not fall through) while a patcher is focused"
        );
        assert_eq!(
            editor.runtime_mut().eval_str("tab-target").unwrap(),
            Some(eseqlisp::vm::Value::String("".to_string())),
            "plain Tab must not leave the patch editor for the arrangement view"
        );
    }

    #[test]
    fn control_h_collapses_expanded_sequencer_tracks() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.sequencer/expanded-track-ids '(0 1))
                (def eseq.sequencer/collapse-all-tracks () (set! eseq.sequencer/expanded-track-ids '()))
                "#,
            )
            .expect("install collapse handler");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("eseq.sequencer/expanded-track-ids")
                .unwrap(),
            Some(eseqlisp::vm::Value::List(vec![]))
        );
    }

    #[test]
    fn platform_primary_a_selects_current_sequencer_track_steps() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer_with_mode("*sequencer*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![("selected-steps", build_selection_value(&selected_steps))],
            true,
        );
        {
            let selected_steps = Arc::clone(&selected_steps);
            editor
                .runtime_mut()
                .register_native("seq-select-all-steps", move |_args, _ctx| {
                    let mut selected_steps = selected_steps.lock().unwrap();
                    selected_steps.clear();
                    selected_steps.extend(0..16);
                    Ok(Value::Number(16.0))
                });
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-core-state/selected-bus 1)
                (defstate select-all-count 0)
                (def eseq.sequencer/select-all-current-track-steps ()
                  (do
                    (set! eseq.seq-core-state/selected-bus -1)
                    (set! select-all-count (+ select-all-count 1))
                    (seq-select-all-steps)))
                "#,
            )
            .expect("install select-all handler");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('a'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("eseq.seq-core-state/selected-bus").unwrap(),
            Some(eseqlisp::vm::Value::Number(-1.0))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("select-all-count").unwrap(),
            Some(eseqlisp::vm::Value::Number(1.0))
        );
        assert_eq!(
            *selected_steps.lock().unwrap(),
            (0..16).collect::<HashSet<_>>(),
            "Cmd+A should select every current-track step; the native's UI invalidation owns the reactive projection",
        );
    }

    /// Modifiers that must reach the global sequencer chords: the platform
    /// primary everywhere, plus the historical Ctrl alias on macOS.
    fn sequencer_shortcut_modifiers() -> Vec<KeyModifiers> {
        let mut modifiers = vec![primary_shortcut_modifier()];
        if CURRENT_SHORTCUT_PLATFORM == ShortcutPlatform::MacOS {
            modifiers.push(KeyModifiers::CONTROL);
        }
        modifiers
    }

    #[test]
    fn platform_primary_g_dispatches_track_grouping() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def eseq.mixer/seq-ctrl-g ()
                  (host-command "group-selected-tracks" (dict)))
                "#,
            )
            .expect("install track group dispatcher");
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        let modifiers = sequencer_shortcut_modifiers();
        for modifiers in &modifiers {
            assert!(handle_metal_command_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Char('g'), *modifiers),
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ));
        }

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), modifiers.len());
        assert!(commands.iter().all(|command| matches!(
            command,
            HostCommand::Custom { name, .. } if name == "group-selected-tracks"
        )));
    }

    /// Cmd/Ctrl+G belongs to track grouping now, so Agent Mode's entry point
    /// is the `C-x a` chord registered by ui/agent.lisp. Prove the chord is
    /// not swallowed by the live-keyboard/`a` handling and reaches the
    /// `agent-open` handler.
    #[test]
    fn control_x_a_chord_opens_the_agent_panel() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (module eseq.agent)
                (def agent-open ()
                  (host-command "agent-open-probe" (dict)))
                (bind-key "C-x a" "agent-open")
                "#,
            )
            .expect("install agent-open binding");
        editor.refresh_runtime_side_effects();
        editor.drain_host_commands();

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        let commands = editor.drain_host_commands();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                HostCommand::Custom { name, .. } if name == "agent-open-probe"
            )),
            "C-x a should reach agent-open, got {commands:?}"
        );
    }

    #[test]
    fn platform_primary_i_queues_new_instrument_editor() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('i'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        let commands = editor.drain_host_commands();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                HostCommand::Custom { name, .. } if name == "enter-new-instrument-editor"
            )),
            "Cmd+i should queue the new instrument editor command: {commands:?}"
        );
    }

    #[test]
    fn platform_primary_d_clones_selected_track_pattern_in_mixer() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*mixer*", "");
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let cloned = Arc::new(AtomicBool::new(false));
        {
            let cloned = Arc::clone(&cloned);
            editor.runtime_mut().register_native(
                "seq-clone-active-track-pattern",
                move |_args, _ctx| {
                    cloned.store(true, Ordering::Relaxed);
                    Ok(Value::Bool(true))
                },
            );
        }
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('d'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(
            cloned.load(Ordering::Relaxed),
            "Cmd+D in the mixer should invoke selected track-pattern clone"
        );
    }

    #[test]
    fn command_or_control_plus_key_and_minus_use_track_pattern_length_actions() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-core-state/selected-bus -1)
                (defstate pattern-length-action "")
                (def eseq.seq-core-state/seq-has-selected-bus? () (>= eseq.seq-core-state/selected-bus 0))
                (def eseq.seq-grid-mode/double-track-pattern () (set! pattern-length-action "double"))
                (def eseq.seq-grid-mode/halve-track-pattern () (set! pattern-length-action "halve"))
                "#,
            )
            .expect("install pattern length handlers");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('+'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("pattern-length-action")
                .unwrap(),
            Some(Value::String("double".to_string()))
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set! pattern-length-action "")"#)
            .expect("reset action");
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('='), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("pattern-length-action")
                .unwrap(),
            Some(Value::String("double".to_string()))
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set! pattern-length-action "")"#)
            .expect("reset action");
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(
                KeyCode::Char('='),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("pattern-length-action")
                .unwrap(),
            Some(Value::String("double".to_string()))
        );

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("pattern-length-action")
                .unwrap(),
            Some(Value::String("halve".to_string()))
        );
    }

    #[test]
    fn control_plus_and_minus_do_not_resize_track_pattern_while_bus_is_selected() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-core-state/selected-bus 0)
                (defstate pattern-length-action "")
                (def eseq.seq-core-state/seq-has-selected-bus? () (>= eseq.seq-core-state/selected-bus 0))
                (def eseq.seq-grid-mode/double-track-pattern () (set! pattern-length-action "double"))
                (def eseq.seq-grid-mode/halve-track-pattern () (set! pattern-length-action "halve"))
                "#,
            )
            .expect("install selected bus fixture");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('+'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("pattern-length-action")
                .unwrap(),
            Some(Value::String("".to_string()))
        );
    }

    #[test]
    fn platform_primary_plus_and_minus_resize_all_patterns_when_drum_rack_bus_is_selected() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.seq-core-state/selected-bus 2)
                (def eseq.seq-core-state/seq-has-selected-bus? () true)
                (def eseq.drum-rack-v2/rack-of-bus (bus) 4)
                "#,
            )
            .expect("install selected rack fixture");
        let calls = Arc::new(Mutex::new(Vec::new()));
        {
            let calls = Arc::clone(&calls);
            editor.runtime_mut().register_native(
                "seq-resize-drum-rack-patterns",
                move |args, _ctx| {
                    calls.lock().unwrap().push(args.to_vec());
                    Ok(Value::Bool(true))
                },
            );
        }
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        for (key, operation) in [('+', "double"), ('-', "halve")] {
            assert!(handle_metal_command_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Char(key), primary_shortcut_modifier()),
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ));
            let calls = calls.lock().unwrap();
            let args = calls.last().expect("rack resize native call");
            assert_eq!(args.first(), Some(&Value::Number(2.0)));
            assert_eq!(args.get(1), Some(&Value::Keyword(operation.to_string())));
        }
    }

    #[test]
    fn platform_primary_m_toggles_current_track_mods_view() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.effects.state/instrument-panel-tab 2)
                (defstate eseq.effects.state/instrument-mods-open false)
                (defstate eseq.seq-core-state/selected-bus 1)
                (def eseq.effects.effect-panels/instrument-toggle-mods-view ()
                  (do
                    (set! eseq.effects.state/instrument-panel-tab 0)
                    (set! eseq.effects.state/instrument-mods-open (not eseq.effects.state/instrument-mods-open))))
                (def eseq.seq-panels/seq-toggle-current-track-mods-view ()
                  (do
                    (set! eseq.seq-core-state/selected-bus -1)
                    (eseq.effects.effect-panels/instrument-toggle-mods-view)))
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
            &KeyEvent::new(KeyCode::Char('m'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("eseq.effects.state/instrument-mods-open")
                .unwrap(),
            Some(eseqlisp::vm::Value::Bool(true))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("eseq.effects.state/instrument-panel-tab")
                .unwrap(),
            Some(eseqlisp::vm::Value::Number(0.0))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("eseq.seq-core-state/selected-bus").unwrap(),
            Some(eseqlisp::vm::Value::Number(-1.0))
        );

        // Ctrl stays a live alias for this chord on macOS, and is the primary
        // everywhere else, so it must toggle the view back off.
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
                .eval_str("eseq.effects.state/instrument-mods-open")
                .unwrap(),
            Some(eseqlisp::vm::Value::Bool(false))
        );
    }

    #[test]
    fn platform_primary_m_toggles_mods_view_outside_ui_buffers() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().read_only = true;
        editor.active_buffer_mut().view_mode = ViewMode::TextOnly;
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (defstate eseq.effects.state/instrument-mods-open false)
                (def eseq.effects.effect-panels/instrument-toggle-mods-view ()
                  (set! eseq.effects.state/instrument-mods-open (not eseq.effects.state/instrument-mods-open)))
                (def eseq.seq-panels/seq-toggle-current-track-mods-view ()
                  (eseq.effects.effect-panels/instrument-toggle-mods-view))
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
            &KeyEvent::new(KeyCode::Char('m'), primary_shortcut_modifier()),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("eseq.effects.state/instrument-mods-open")
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
                (def eseq.seq-panels/seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
                (def eseq.step-grid-interactions/cursor-left () (set! cursor-left-count (+ cursor-left-count 1)))
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
                (def eseq.seq-panels/seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
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
                (def eseq.seq-panels/seq-toggle-main-or-piano-roll () (set! tab-target "piano"))
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

    #[test]
    fn sequencer_soft_number_entry_edits_current_expanded_track_step() {
        fn number_list(values: &[f64]) -> Value {
            Value::List(
                values
                    .iter()
                    .copied()
                    .map(|value| Rc::new(RefCell::new(Value::Number(value))))
                    .collect(),
            )
        }

        fn list(values: Vec<Value>) -> Value {
            Value::List(
                values
                    .into_iter()
                    .map(|value| Rc::new(RefCell::new(value)))
                    .collect(),
            )
        }

        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.set_layout_viewport(80, 20);
        let initial_values = number_list(&[1.0; 16]);
        editor.open_scratch_buffer_with_mode("*sequencer*", "", BufferMode::ESeqLisp);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("velocities", initial_values.clone()),
                ("track-velocities", list(vec![initial_values])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def eseq.seq-core-state/seq-has-selected-bus? () false)
                (def eseq.sequencer/current-selected-step () 2)
                (def eseq.sequencer/current-param-mode () 0)
                (def eseq.sequencer/current-number-picker-key () "seqv-expanded-param-number-picker-0")
                (defstate seqv-soft-edit-flushed 1)
                (defstate cursor-toggle-count 0)
                (def eseq.step-grid-interactions/cursor-toggle () (set! cursor-toggle-count (+ cursor-toggle-count 1)))
                (effect
                  (set! seqv-soft-edit-flushed (nth (nth SEQ.track-velocities 0) 2)))
                "#,
            )
            .expect("install sequencer soft edit fixture");
        let tree = editor
            .runtime_mut()
            .eval_str(
                r#"(number-picker :key "seqv-expanded-param-number-picker-0"
                    :value 1 :min 0 :max 1 :decimals 2
                    :width 8 :height 1.3 :font-size 11)"#,
            )
            .expect("build sequencer number picker")
            .expect("number picker should produce a widget tree");
        editor
            .active_buffer_mut()
            .set_widget_tree(Some(tree.clone()), None);
        editor.runtime_mut().set_widget_tree(tree);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .widget_layout()
            .expect("sequencer number picker should lay out");

        let state = Arc::new(SequencerState::new(1, vec![]));
        let mut app = soft_edit_test_app(Arc::clone(&state));
        let current_track = Arc::new(AtomicUsize::new(0));
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        expanded_step_projection.set_viewport(ExpandedStepViewport {
            track: 0,
            track_id: 0,
            page: 0,
            mode: 0,
            cursor_step: 2,
        });
        let mut edit = SoftStepParamEdit::default();

        for key in [
            KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ] {
            assert!(
                handle_metal_soft_step_param_key(
                    &mut editor,
                    &key,
                    &mut app,
                    &current_track,
                    &expanded_step_projection,
                    &mut edit,
                ),
                "sequencer soft edit should consume {key:?}"
            );
        }

        assert_eq!(
            state.pattern.step_data[0].get(2, StepParam::Velocity),
            0.5,
            "sequencer numeric entry should commit through the same soft number-picker path"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(nth SEQ.velocities 2)")
                .unwrap(),
            Some(Value::Number(0.5)),
            "soft edit should keep the current-track parameter mirror in sync"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(nth (nth SEQ.track-velocities 0) 2)")
                .unwrap(),
            Some(Value::Number(0.5)),
            "soft edit should keep the all-track sequencer parameter mirror in sync"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("seqv-soft-edit-flushed")
                .unwrap(),
            Some(Value::Number(0.5)),
            "soft edit commit should flush the reactive cycle immediately"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str(r#"(reactive-get "SEQ" "seqv-slot-param-slider-0-0-2")"#)
                .unwrap(),
            Some(Value::Number(0.5)),
            "soft edit commit should update the visible expanded slider slot immediately"
        );
    }

    #[test]
    fn sequencer_soft_number_entry_edits_current_process_lane_step() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let mut app = soft_edit_test_app(Arc::clone(&state));
        let current_track = Arc::new(AtomicUsize::new(0));
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.set_layout_viewport(80, 20);
        editor.open_scratch_buffer_with_mode("*sequencer*", "", BufferMode::ESeqLisp);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("process-lanes", Value::List(vec![])),
                ("track-process-lanes", Value::List(vec![])),
                ("process-slots", Value::List(vec![])),
                ("track-process-slots", Value::List(vec![])),
                ("process-library", Value::List(vec![])),
            ],
            true,
        );
        sequencer::lisp_host::register_published_process_authoring_natives(
            editor.runtime_mut(),
            Arc::clone(&state),
            Arc::new(AtomicUsize::new(0)),
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def eseq.seq-core-state/seq-has-selected-bus? () false)
                (def eseq.sequencer/current-selected-step () 2)
                (def eseq.sequencer/current-param-mode () 7)
                (def eseq.sequencer/current-number-picker-key () "seqv-expanded-param-number-picker-0")
                (defstate cursor-toggle-count 0)
                (def eseq.step-grid-interactions/cursor-toggle () (set! cursor-toggle-count (+ cursor-toggle-count 1)))

                (def-accumulator sparse-transpose
                  :target (step-param :transpose)
                  :amount (amount :lane true :default 0)
                  :range (-24 24)
                  :mode :clip)

                (def climb
                  (processes :track 0
                    (sparse-transpose :amount (lane 0 1 0 0))))
                "#,
            )
            .expect("install process lane soft edit fixture");
        let tree = editor
            .runtime_mut()
            .eval_str(
                r#"(number-picker :key "seqv-expanded-param-number-picker-0"
                    :value 0 :min -24 :max 24 :decimals 0
                    :width 8 :height 1.3 :font-size 11)"#,
            )
            .expect("build process lane number picker")
            .expect("number picker should produce a widget tree");
        editor
            .active_buffer_mut()
            .set_widget_tree(Some(tree.clone()), None);
        editor.runtime_mut().set_widget_tree(tree);
        editor
            .widget_layout()
            .expect("process lane number picker should lay out");

        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        expanded_step_projection.set_viewport(ExpandedStepViewport {
            track: 0,
            track_id: 0,
            page: 0,
            mode: PROCESS_LANE_MODE_OFFSET,
            cursor_step: 2,
        });
        let mut edit = SoftStepParamEdit::default();

        for key in [
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ] {
            assert!(
                handle_metal_soft_step_param_key(
                    &mut editor,
                    &key,
                    &mut app,
                    &current_track,
                    &expanded_step_projection,
                    &mut edit,
                ),
                "process lane soft edit should consume {key:?}"
            );
        }

        let chain = state
            .track_process_chain(0)
            .expect("track 0 process chain should exist");
        let amount_lane = chain.slots[0]
            .lanes
            .get("amount")
            .expect("amount lane should exist");
        assert_eq!(
            amount_lane.values.get(2).copied(),
            Some(2.0),
            "sequencer numeric entry should update the process lane at the cursor step"
        );
        assert_eq!(
            state.pattern.step_data[0].get(2, StepParam::Transpose),
            0.0,
            "process lane soft edits must not mutate built-in step data"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str(r#"(reactive-get "SEQ" "seqv-slot-param-slider-0-7-2")"#)
                .unwrap(),
            Some(Value::Number(2.0)),
            "soft edit commit should update the visible process lane slider slot immediately"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("cursor-toggle-count")
                .unwrap(),
            Some(Value::Number(0.0)),
            "soft Enter commit should not fall through to the step gate toggle"
        );
    }

    #[test]
    fn sequencer_soft_enter_commits_existing_number_picker_edit() {
        fn number_list(values: &[f64]) -> Value {
            Value::List(
                values
                    .iter()
                    .copied()
                    .map(|value| Rc::new(RefCell::new(Value::Number(value))))
                    .collect(),
            )
        }

        fn list(values: Vec<Value>) -> Value {
            Value::List(
                values
                    .into_iter()
                    .map(|value| Rc::new(RefCell::new(value)))
                    .collect(),
            )
        }

        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.set_layout_viewport(80, 20);
        let initial_values = number_list(&[1.0; 16]);
        editor.open_scratch_buffer_with_mode("*sequencer*", "", BufferMode::ESeqLisp);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("velocities", initial_values.clone()),
                ("track-velocities", list(vec![initial_values])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def eseq.seq-core-state/seq-has-selected-bus? () false)
                (def eseq.sequencer/current-selected-step () 2)
                (def eseq.sequencer/current-param-mode () 0)
                (def eseq.sequencer/current-number-picker-key () "seqv-expanded-param-number-picker-0")
                (defstate seqv-soft-edit-flushed 1)
                (defstate cursor-toggle-count 0)
                (def eseq.step-grid-interactions/cursor-toggle () (set! cursor-toggle-count (+ cursor-toggle-count 1)))
                (effect
                  (set! seqv-soft-edit-flushed (nth (nth SEQ.track-velocities 0) 2)))
                "#,
            )
            .expect("install sequencer soft edit fixture");
        let tree = editor
            .runtime_mut()
            .eval_str(
                r#"(number-picker :key "seqv-expanded-param-number-picker-0"
                    :value 1 :min 0 :max 1 :decimals 2
                    :width 8 :height 1.3 :font-size 11)"#,
            )
            .expect("build sequencer number picker")
            .expect("number picker should produce a widget tree");
        editor
            .active_buffer_mut()
            .set_widget_tree(Some(tree.clone()), None);
        editor.runtime_mut().set_widget_tree(tree);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor
            .widget_layout()
            .expect("sequencer number picker should lay out");

        let widget_id = current_step_param_number_picker_id(&mut editor)
            .expect("current sequencer parameter number picker id");
        for key in [
            KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE),
        ] {
            assert!(
                handle_number_picker_edit_key_for_widget(
                    widget_id,
                    WidgetKeyEvent {
                        code: key.code,
                        modifiers: key.modifiers,
                    },
                    1.0,
                    0.0,
                    1.0,
                    2,
                )
                .is_some(),
                "fixture should arm number-picker edit state with {key:?}"
            );
        }

        let state = Arc::new(SequencerState::new(1, vec![]));
        let mut app = soft_edit_test_app(Arc::clone(&state));
        let current_track = Arc::new(AtomicUsize::new(0));
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        expanded_step_projection.set_viewport(ExpandedStepViewport {
            track: 0,
            track_id: 0,
            page: 0,
            mode: 0,
            cursor_step: 2,
        });
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));
        let mut edit = SoftStepParamEdit::default();

        assert!(
            !handle_metal_command_shortcut(
                &mut editor,
                &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "global Enter should not toggle the cursor step while the current parameter picker is editing"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("cursor-toggle-count")
                .unwrap(),
            Some(Value::Number(0.0)),
            "global Enter should leave cursor-toggle untouched while the picker edit is pending"
        );

        assert!(
            handle_metal_soft_step_param_key(
                &mut editor,
                &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut app,
                &current_track,
                &expanded_step_projection,
                &mut edit,
            ),
            "Enter should commit an already-editing sequencer number picker instead of falling through to cursor-toggle"
        );

        assert_eq!(
            state.pattern.step_data[0].get(2, StepParam::Velocity),
            0.25,
            "Enter should commit the pending number-picker edit through the soft step path"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("seqv-soft-edit-flushed")
                .unwrap(),
            Some(Value::Number(0.25)),
            "commit should flush the reactive mirror immediately"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str(r#"(reactive-get "SEQ" "seqv-slot-param-slider-0-0-2")"#)
                .unwrap(),
            Some(Value::Number(0.25)),
            "Enter commit should update the visible expanded slider slot immediately"
        );
    }
}
