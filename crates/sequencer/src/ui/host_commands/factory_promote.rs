//! Promote to factory (`content/ui/factory-promote.lisp`, eseq-jhmx). The M-x
//! commands send `factory-promote-open` with a kind; Rust captures the target
//! and vets its dependencies, and the modal shows the name field plus every
//! dependency that will be skipped. `factory-promote-commit` writes the
//! object into the checkout's `content/` tree.

use crate::*;
use sequencer::app::PromoteTarget;

pub(super) const COMMANDS: &[&str] = &["factory-promote-open", "factory-promote-commit"];

thread_local! {
    /// The target the open modal describes, so Promote writes what the
    /// modal showed even if the cursor moved while it was open.
    static TARGET: std::cell::Cell<Option<PromoteTarget>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn register_state(runtime: &mut eseqlisp::Runtime) {
    runtime.register_reactive("FACTORY_PROMOTE", vec![
        ("kind", Value::String(String::new())),
        ("destination", Value::String(String::new())),
        ("skipped", Value::List(Vec::new())),
        ("blocking", Value::String(String::new())),
        ("error", Value::String(String::new())),
        // The name a commit found already taken; Promote then replaces it.
        ("taken", Value::String(String::new())),
    ], true);
}

fn open(
    kind: &str,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let current_track = ctx.shared.current_track.clone();
    let _ = current_track_for_app(app, &current_track);
    let target = app.factory_promote_target(kind)?;
    let preview = app.preview_factory_promotion(target);
    TARGET.with(|cell| cell.set(Some(target)));

    if !editor.switch_active_tile_to_buffer_named("*arrangement*") {
        editor.switch_active_tile_to_buffer_named("*sequencer*");
    }
    let rt = editor.runtime_mut();
    let kind_label = if preview.kind_label.is_empty() { kind.to_string() } else { preview.kind_label };
    rt.set_reactive("FACTORY_PROMOTE", "kind", Value::String(kind_label));
    rt.set_reactive("FACTORY_PROMOTE", "destination", Value::String(preview.destination));
    rt.set_reactive("FACTORY_PROMOTE", "skipped", build_string_list(&preview.skipped));
    rt.set_reactive(
        "FACTORY_PROMOTE",
        "blocking",
        Value::String(preview.blocking.unwrap_or_default()),
    );
    rt.set_reactive("FACTORY_PROMOTE", "error", Value::String(String::new()));
    rt.set_reactive("FACTORY_PROMOTE", "taken", Value::String(String::new()));
    let open = rt
        .global_value("eseq.factory-promote/open")
        .ok_or("Promote UI is unavailable")?;
    rt.invoke(open, vec![Value::String(preview.default_name)])
        .map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn commit(payload: &Value, app: &mut app::App, editor: &mut Editor) -> Result<(), String> {
    let target = TARGET.with(|cell| cell.get()).ok_or("Open a promotion first")?;
    let name = extract_string_from_payload(payload, "name").unwrap_or_default();
    let overwrite = extract_bool_from_payload(payload, "overwrite");
    match app.promote_to_factory(target, &name, overwrite) {
        Ok((path, report)) => {
            TARGET.with(|cell| cell.set(None));
            let rt = editor.runtime_mut();
            rt.eval_str("(eseq.factory-promote/close)").map_err(|error| format!("{error:?}"))?;
            record_preset_listings();
            sync_sidebar_browser(rt, app, app.ui.cursor_track);
            let file = path
                .file_name()
                .map(|file| file.to_string_lossy().to_string())
                .unwrap_or_default();
            let message = match report.skipped.len() {
                0 => format!("Promoted '{}' to factory ({file})", name.trim()),
                n => format!(
                    "Promoted '{}' to factory ({file}), {n} dependenc{} skipped",
                    name.trim(),
                    if n == 1 { "y" } else { "ies" }
                ),
            };
            editor.handle_host_event(HostEvent::Status(format!("{message}: {}", path.display())));
            editor.show_toast(message, eseqlisp::ToastKind::Success);
            Ok(())
        }
        Err(error) => {
            if error.contains("already exists") {
                editor
                    .runtime_mut()
                    .set_reactive("FACTORY_PROMOTE", "taken", Value::String(name.trim().to_string()));
            }
            Err(error)
        }
    }
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match name {
        "factory-promote-open" => {
            let kind = extract_string_from_payload(&payload, "kind").unwrap_or_default();
            open(&kind, app, editor, ctx)
        }
        "factory-promote-commit" => commit(&payload, app, editor),
        _ => Ok(()),
    };
    if let Err(error) = result {
        editor
            .runtime_mut()
            .set_reactive("FACTORY_PROMOTE", "error", Value::String(error.clone()));
        editor.show_transient_message(error);
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}
