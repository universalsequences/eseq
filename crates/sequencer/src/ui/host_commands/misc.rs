use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-record-quantize",
    "toggle-metronome",
    "toggle-roll-mode",
];

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
            let enabled = !state.transport.roll_mode.fetch_xor(true, Ordering::AcqRel);
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
