use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-record-quantize",
    "toggle-metronome",
    "toggle-roll-mode",
];

pub(crate) fn toggle_roll_mode(state: &SequencerState, editor: &mut Editor) -> bool {
    let enabled = !state.transport.roll_mode.fetch_xor(true, Ordering::AcqRel);
    eprintln!("[roll-debug] host-command toggle-roll-mode enabled={enabled}");
    if !enabled {
        // Toggling roll mode off always clears stuck rolls
        // (docs/rolling-core-spec.md 7).
        state
            .transport
            .sequence_rolling
            .store(false, Ordering::Release);
        state.push_roll_command(sequencer::sequencer::RollCommand::ClearAll);
    }
    editor
        .runtime_mut()
        .set_reactive("SEQ", "roll-mode", Value::Bool(enabled));
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    enabled
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
            editor.runtime_mut().set_reactive(
                "SEQ",
                "record-quantize",
                Value::String(quantize.transport_label().to_string()),
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "toggle-roll-mode" => {
            toggle_roll_mode(&state, editor);
        }
        "toggle-metronome" => {
            let enabled = !state
                .transport
                .metronome_enabled
                .fetch_xor(true, Ordering::AcqRel);
            editor
                .runtime_mut()
                .set_reactive("SEQ", "metronome", Value::Bool(enabled));
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
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
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![("roll-mode", Value::Bool(false))],
            true,
        );
        let mut editor = Editor::new(runtime, EditorConfig::default());

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
