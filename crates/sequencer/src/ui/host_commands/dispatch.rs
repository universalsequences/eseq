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
        n if super::rack::COMMANDS.contains(&n) => super::rack::handle(name, payload, app, editor, ctx),
        n if super::instrument_params::COMMANDS.contains(&n) => super::instrument_params::handle(name, payload, app, editor, ctx),
        n if super::effects::COMMANDS.contains(&n) => super::effects::handle(name, payload, app, editor, ctx),
        n if super::bus_steps::COMMANDS.contains(&n) => super::bus_steps::handle(name, payload, app, editor, ctx),
        n if super::routing::COMMANDS.contains(&n) => super::routing::handle(name, payload, app, editor, ctx),
        n if super::samples::COMMANDS.contains(&n) => super::samples::handle(name, payload, app, editor, ctx),
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
