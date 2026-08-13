use super::*;
use eseqlisp::widget_render::number_picker::{
    clear_number_picker_edit_state, handle_number_picker_edit_key_for_widget,
    number_picker_edit_state, NumberPickerEditOutcome,
};

#[derive(Clone, Debug)]
pub(crate) struct HeldKeyboardNote {
    key: char,
    transpose: f32,
    positions: Vec<(usize, sequencer::sequencer::RecordPosition)>,
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

    #[cfg(target_os = "macos")]
    let primary = KeyModifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let primary = KeyModifiers::CONTROL;

    if !key.modifiers.contains(primary) || key.modifiers.contains(KeyModifiers::ALT) {
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
    if key.modifiers != crossterm::event::KeyModifiers::SUPER {
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
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(
        (key.code, key.modifiers),
        (KeyCode::Char('f') | KeyCode::Char('F'), KeyModifiers::SUPER)
    )
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PatternLengthShortcut {
    Double,
    Halve,
}

fn pattern_length_shortcut(key: &crossterm::event::KeyEvent) -> Option<PatternLengthShortcut> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let has_shortcut_modifier = key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
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
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(
        (key.code, key.modifiers),
        (KeyCode::Char(','), KeyModifiers::SUPER)
    )
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
            if mode == "instrument-patcher" || mode == "instrument-patcher-source"
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
    use super::widget_type_captures_text_input;

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

/// Cmd-C / Cmd-V / Cmd-D / Backspace over the arrangement. Returns true when
/// the key was consumed.
///
/// Backspace only takes the key for a MARQUEE region (region set, no clip
/// selected). A clip click also sets a one-clip region (spec 4.1 as amended),
/// and that case must keep falling through to the existing clip-delete path.
fn handle_arrangement_region_shortcut(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
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
            if modifiers.contains(KeyModifiers::SUPER) =>
        {
            if !published_value_is_set(editor, "SEQ.song-region") {
                return false;
            }
            enqueue_region_command(editor, "song-region-copy");
            true
        }
        (KeyCode::Char('v') | KeyCode::Char('V'), modifiers)
            if modifiers.contains(KeyModifiers::SUPER) =>
        {
            // No payload: the command pastes at the mirrored arrangement
            // cursor, floored to the clipboard's own grid. An empty clipboard
            // reports on the status line rather than falling through to a
            // shortcut that means nothing here.
            enqueue_region_command(editor, "song-region-paste");
            true
        }
        (KeyCode::Char('d') | KeyCode::Char('D'), modifiers)
            if modifiers.contains(KeyModifiers::SUPER) =>
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
        // clip): the widget's own delete handling wins over the global
        // selected-step shortcut, mirroring the navigation-key gate below.
        && editor.focused_widget_id().is_none()
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
                let next = if key.code == KeyCode::Up {
                    if current == 0 {
                        track_count - 1
                    } else {
                        current - 1
                    }
                } else {
                    (current + 1) % track_count
                };
                select_track_for_edit(editor, next);
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
                return true;
            }
            _ => {}
        }
    }

    if editor.minibuffer_prompt().is_none()
        && editor.prompt_text().is_none()
        && active_buffer_accepts_global_ui_shortcuts(editor)
        && !focused_widget_captures_text_input(editor)
    {
        if let Some(shortcut) = pattern_length_shortcut(key) {
            if metal_has_selected_bus(editor) {
                return false;
            }
            let command = match shortcut {
                PatternLengthShortcut::Double => "(eseq.seq-grid-mode/double-track-pattern)",
                PatternLengthShortcut::Halve => "(eseq.seq-grid-mode/halve-track-pattern)",
            };
            let _ = editor.runtime_mut().eval_str(command);
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
            return true;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('a') | KeyCode::Char('A'), modifiers)
                if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
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
            (KeyCode::Char('i') | KeyCode::Char('I'), KeyModifiers::SUPER) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(host-command \"enter-new-instrument-editor\" (dict))");
                editor.refresh_runtime_side_effects();
                return true;
            }
            (KeyCode::Char('d') | KeyCode::Char('D'), KeyModifiers::SUPER)
                if editor.active_buffer().name == "*mixer*" =>
            {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(seq-clone-active-track-pattern)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            _ => {}
        }
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'))
    {
        // A 2+ track multi-selection only exists via mixer cmd-click, so C-g
        // groups when one is present; otherwise it opens the agent. The Lisp
        // dispatcher (seq-ctrl-g) decides.
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
    /// Notes were written into a pending take (takes spec 8.4): the live
    /// pattern is untouched, so no step-grid rebuild and no live-recording
    /// history transaction.
    RecordedTake,
}

impl RecordingKeyOutcome {
    pub(crate) fn consumed(self) -> bool {
        !matches!(self, Self::Ignored)
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

/// Intercept keyboard events for live recording.
pub(crate) fn handle_recording_key(
    key: &crossterm::event::KeyEvent,
    app: &mut sequencer::app::App,
    state: &Arc<SequencerState>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    recording: &Arc<AtomicBool>,
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    keyboard_octave: &Arc<std::sync::atomic::AtomicI32>,
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
            let press_time = Instant::now();
            let mut positions = Vec::new();

            // Send note-on to audio thread for all armed tracks
            for (track, a) in armed.iter().enumerate() {
                if *a {
                    pressed_tracks.push(track);
                    let position = state
                        .record_position_at_instant(track, press_time)
                        .unwrap_or_else(|| sequencer::sequencer::RecordPosition {
                            step: state.transport.track_playheads[track].load(Ordering::Relaxed)
                                as usize,
                            phase: f32::from_bits(
                                state.transport.track_playhead_phases[track]
                                    .load(Ordering::Relaxed),
                            )
                            .clamp(0.0, 1.0),
                        });
                    positions.push((track, position));
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track,
                        transpose,
                        velocity: 1.0,
                        note_off: false,
                    });
                }
            }

            held.push(HeldKeyboardNote {
                key: c,
                transpose,
                positions,
                press_time,
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
                    let bpm = state.transport.bpm.load(Ordering::Relaxed) as f64;
                    let secs_per_step = 60.0 / bpm / 4.0;
                    let hold_secs = note.press_time.elapsed().as_secs_f64();
                    let duration_steps = (hold_secs / secs_per_step).max(0.15).min(64.0) as f32;
                    let mut recorded = false;

                    let mut recorded_take = false;
                    let quantize = sequencer::record_quantize::RecordQuantize::from_atomic(
                        state.transport.record_quantize.load(Ordering::Relaxed) as u8,
                    );
                    // Recording engaged mid-playback: stamp the recording
                    // kind from the active view (unified-transport spec 5) —
                    // arrangement view promotes into arrangement capture so
                    // this performance records as a take; the session view
                    // stamps loop overdub into the looping live pattern.
                    if !note.positions.is_empty() {
                        app.stamp_recording_kind_for_note();
                    }
                    let song_authority = app.song_playback_authority_active();
                    let overdub = app.recording_kind
                        == Some(sequencer::app::song_transport::RecordingKind::Overdub);
                    for (track, position) in &note.positions {
                        // Song-mode take recording (takes spec 8.4): while
                        // arrangement capture is active, an armed track's
                        // notes retarget into its pending take at
                        // clip-relative positions stamped on the
                        // latency-compensated record clock — the live
                        // pattern is NOT written.
                        if !overdub
                            && app.take_record_note(
                                *track,
                                note.press_time,
                                note.transpose,
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
                            if !app.claim_overdub_lane(*track) {
                                continue;
                            }
                        }
                        let num_steps = state.pattern.track_params[*track].get_num_steps();
                        let (local_step, delay) = quantized_record_position(
                            position.step,
                            position.phase,
                            num_steps,
                            state.pattern.track_params[*track].get_timebase(),
                            quantize,
                        );
                        if !state.pattern.patterns[*track].is_active(local_step) {
                            state.pattern.patterns[*track].toggle_step(local_step);
                        }
                        state.pattern.chord_data[*track].add_note_with_timing(
                            local_step,
                            note.transpose,
                            duration_steps,
                            delay,
                        );
                        let first_note = state.pattern.chord_data[*track].get(local_step, 0);
                        state.pattern.step_data[*track].set(
                            local_step,
                            StepParam::Transpose,
                            first_note,
                        );
                        state.pattern.step_data[*track].set(local_step, StepParam::Velocity, 1.0);
                        state.pattern.step_data[*track].set(
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
        build_selection_value, current_step_param_number_picker_id, handle_metal_command_shortcut,
        handle_metal_soft_step_param_key, handle_number_picker_edit_key_for_widget,
        held_note_for_key, note_from_key, quantized_record_position,
        sequencer_history_shortcut, ExpandedStepProjectionRegistry, ExpandedStepViewport,
        HeldKeyboardNote, SequencerHistoryShortcut, SoftStepParamEdit, PROCESS_LANE_MODE_OFFSET,
    };
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use eseqlisp::editor::ViewMode;
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
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
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
            transpose: 0.0,
            positions: vec![(
                0,
                RecordPosition {
                    step: 0,
                    phase: 0.0,
                },
            )],
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

    #[test]
    fn command_comma_toggles_trigger_recording_without_claiming_plain_comma() {
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
            &KeyEvent::new(KeyCode::Char(','), KeyModifiers::SUPER),
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
    fn command_f_focuses_sample_browser_search_from_ui_buffers() {
        let mut editor = sample_browser_keyboard_editor();
        let (state, current_track, selected_steps, step_clipboard) = empty_command_state();

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::SUPER),
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
            &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::SUPER),
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
            &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::SUPER),
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
            &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::SUPER),
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
    fn cmd_number_selects_visible_sequencer_step_tab() {
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
            &KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SUPER),
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
    fn command_or_control_a_selects_current_sequencer_track_steps() {
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
            &KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER),
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

    #[test]
    fn command_i_queues_new_instrument_editor() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard: Arc<Mutex<Option<(usize, Vec<(usize, StepSnapshot)>)>>> =
            Arc::new(Mutex::new(None));

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('i'), KeyModifiers::SUPER),
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
    fn command_d_clones_selected_track_pattern_in_mixer() {
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
            &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::SUPER),
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
            &KeyEvent::new(KeyCode::Char('='), KeyModifiers::SUPER),
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
    fn command_or_control_m_toggles_current_track_mods_view() {
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
            &KeyEvent::new(KeyCode::Char('m'), KeyModifiers::SUPER),
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
    fn command_m_toggles_mods_view_outside_ui_buffers() {
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
            &KeyEvent::new(KeyCode::Char('m'), KeyModifiers::SUPER),
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
