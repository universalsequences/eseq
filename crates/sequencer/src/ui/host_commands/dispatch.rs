use crate::*;

/// Routes one drained `HostCommand::Custom` command to its domain module.
/// `return` inside handlers corresponds to the old `continue` targeting the
/// event loop's drain loop (nothing follows dispatch in that loop arm).
pub(crate) fn dispatch_custom_host_command(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    match name {
        n if super::step_history::COMMANDS.contains(&n) => super::step_history::handle(name, payload, app, editor, ctx),
        n if super::tracks::COMMANDS.contains(&n) => super::tracks::handle(name, payload, app, editor, ctx),
        n if super::scenes::COMMANDS.contains(&n) => super::scenes::handle(name, payload, app, editor, ctx),
        n if super::song::COMMANDS.contains(&n) => super::song::handle(name, payload, app, editor, ctx),
        n if super::rack::COMMANDS.contains(&n) => super::rack::handle(name, payload, app, editor, ctx),
        n if super::drum_rack_v2::COMMANDS.contains(&n) => super::drum_rack_v2::handle(name, payload, app, editor, ctx),
        n if super::instrument_params::COMMANDS.contains(&n) => super::instrument_params::handle(name, payload, app, editor, ctx),
        n if super::learn::COMMANDS.contains(&n) => super::learn::handle(name, payload, app, editor, ctx),
        n if super::effects::COMMANDS.contains(&n) => super::effects::handle(name, payload, app, editor, ctx),
        n if super::routing::COMMANDS.contains(&n) => super::routing::handle(name, payload, app, editor, ctx),
        n if super::samples::COMMANDS.contains(&n) => super::samples::handle(name, payload, app, editor, ctx),
        n if super::sampler_slices::COMMANDS.contains(&n) => super::sampler_slices::handle(name, payload, app, editor, ctx),
        n if super::scripts::COMMANDS.contains(&n) => super::scripts::handle(name, payload, app, editor, ctx),
        n if super::agent::COMMANDS.contains(&n) => super::agent::handle(name, payload, app, editor, ctx),
        n if super::project::COMMANDS.contains(&n) => super::project::handle(name, payload, app, editor, ctx),
        n if super::instrument_authoring::COMMANDS.contains(&n) => super::instrument_authoring::handle(name, payload, app, editor, ctx),
        n if super::misc::COMMANDS.contains(&n) => super::misc::handle(name, payload, app, editor, ctx),
        other => {
            editor.handle_host_event(HostEvent::Status(format!(
                "Unknown host command: {other}"
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sampler_slice_edit_command_is_registered_with_its_handler() {
        assert!(super::super::sampler_slices::COMMANDS.contains(&"edit-sampler-slice"));
    }

    #[test]
    fn sampler_range_batch_commands_are_registered_with_their_handler() {
        for command in [
            "set-instrument-param-batch",
            "set-instrument-plock-batch",
            "set-instrument-key-lock-batch",
            "set-rack-slot-instrument-param-batch",
            "set-rack-slot-instrument-plock-batch",
        ] {
            assert!(
                super::super::rack::COMMANDS.contains(&command),
                "{command} has an implementation in rack::handle and must be registered for dispatch"
            );
        }
    }
}
