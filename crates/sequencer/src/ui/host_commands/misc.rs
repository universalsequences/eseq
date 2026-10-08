use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-record-quantize",
    "set-metronome",
    "set-roll-mode",
    "set-scroll-inertia",
    "toggle-roll-mode",
];

/// The bool payload of `name`, or an error event and `None`.
fn expect_bool(payload: &Value, name: &str, editor: &mut Editor) -> Option<bool> {
    match payload {
        Value::Bool(on) => Some(*on),
        _ => {
            editor.handle_host_event(HostEvent::Error(format!("{name} expects true or false")));
            None
        }
    }
}

/// Turn roll mode on or off; nothing happens when it already is. Returns
/// whether it changed.
pub(super) fn set_roll_mode(state: &SequencerState, editor: &mut Editor, on: bool) -> bool {
    if state.transport.roll_mode.swap(on, Ordering::AcqRel) == on {
        return false;
    }
    if !on {
        // Turning roll mode off always clears stuck rolls
        // (docs/rolling-core-spec.md 7).
        state.push_roll_command(sequencer::sequencer::RollCommand::ClearAll);
    }
    // The host kinds' next tick pushes `transport.roll-mode`: draw it.
    editor.mark_needs_redraw();
    true
}

pub(super) fn toggle_roll_mode(state: &SequencerState, editor: &mut Editor) -> bool {
    let on = !state.transport.roll_mode.load(Ordering::Acquire);
    set_roll_mode(state, editor, on);
    on
}

/// Turn the metronome on or off; nothing happens when it already is.
fn set_metronome(state: &SequencerState, editor: &mut Editor, on: bool) {
    if state.transport.metronome_enabled.swap(on, Ordering::AcqRel) != on {
        // The next tick pushes `transport.metronome`.
        editor.mark_needs_redraw();
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn handle(
    name: &str,
    payload: Value,
    _app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let state = ctx.shared.state.clone();
    match name {
        "set-record-quantize" => {
            let Value::String(label) = payload else {
                editor.handle_host_event(HostEvent::Error(
                    "Record quantization selection was invalid".to_string(),
                ));
                return;
            };
            let Some(quantize) =
                sequencer::record_quantize::RecordQuantize::from_transport_label(
                    &label,
                )
            else {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Unknown record quantization: {label}"
                )));
                return;
            };
            state
                .transport
                .record_quantize
                .store(quantize as u32, Ordering::Release);
            // The next tick pushes `transport.record-quantize`.
            editor.mark_needs_redraw();
        }
        "set-scroll-inertia" => {
            // App-side scroll momentum, for compositors that provide none
            // (Wayland). Opt-in from init.lisp:
            //   (host-command "set-scroll-inertia" true)
            if let Some(enabled) = expect_bool(&payload, name, editor) {
                ctx.gesture.scroll_inertia.set_enabled(enabled);
            }
        }
        "toggle-roll-mode" => {
            toggle_roll_mode(&state, editor);
        }
        // Absolute forms of the toggles (`transport.roll-mode` and
        // `transport.metronome` :set): nothing happens when already so.
        "set-roll-mode" => {
            if let Some(on) = expect_bool(&payload, name, editor) {
                set_roll_mode(&state, editor, on);
            }
        }
        "set-metronome" => {
            if let Some(on) = expect_bool(&payload, name, editor) {
                set_metronome(&state, editor, on);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::EditorConfig;

    #[test]
    fn direct_roll_toggle_updates_transport_without_deferred_host_dispatch() {
        let state = SequencerState::new(1, vec![]);
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());

        assert!(toggle_roll_mode(&state, &mut editor));
        assert!(state.transport.roll_mode.load(Ordering::Acquire));
        assert!(!toggle_roll_mode(&state, &mut editor));
        assert!(!state.transport.roll_mode.load(Ordering::Acquire));
        assert!(matches!(
            state.drain_roll_commands().as_slice(),
            [sequencer::sequencer::RollCommand::ClearAll]
        ));
    }
}
