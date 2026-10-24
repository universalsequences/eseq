use crate::*;

/// Controlled import from another project (docs/rig-spec.md §8): the
/// Projects peek's import button. The work lives in
/// `crate::project_import_ui`.
pub(super) const COMMANDS: &[&str] = &["import-from-project"];

pub(super) fn handle(
    name: &str,
    payload: Value,
    _app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    if name == "import-from-project" {
        crate::project_import_ui::handle_import_from_project(payload, editor, ctx);
    }
}
