//! Program scripts drive draft tracks (docs/program-generators-spec.md §2-3).
//! C-c C-c in a script buffer evaluates it; when its last value is a program
//! (an `eseq.gen` program, such as an `eseq.modal` patch) the generated
//! instrument plays on that buffer's own draft track: the first evaluation
//! adds the track, later ones recompile it in place (a file-backed buffer
//! keeps its track by path). The draft is an ordinary track in the normal
//! layout; no editor session takes over the screen.

use crate::*;
use sequencer::lisp_host::GeneratedInstrument;

pub(super) const COMMANDS: &[&str] = &["draft-save-open", "draft-save"];

/// A draft track one script buffer drives.
pub(crate) struct Draft {
    /// The buffer's file path, or `buffer:<name>` for an unsaved buffer.
    pub(crate) key: String,
    pub(crate) engine_id: usize,
    /// Holds the generated dsp.lisp / ui.lisp / instrument.json and the
    /// script (source.lisp), and is the draft's asset base.
    pub(crate) dir: PathBuf,
    pub(crate) generation: u64,
    /// What the engine running now was generated as; the engine runs as
    /// `draft/<name>`. The panel shows its ui, not the file, which a pending
    /// compile has already overwritten, and Save writes it.
    pub(crate) instrument: GeneratedInstrument,
}

pub(crate) struct PendingDraftCompile {
    key: String,
    generation: u64,
    /// Recorded on the draft once the compile applies.
    instrument: GeneratedInstrument,
    receiver: std::sync::mpsc::Receiver<PreviewCompileOutcome>,
}

/// The instrument panel tells a draft by this prefix (`draft?` in
/// content/ui/effects/instrument-panel.lisp).
fn draft_engine_name(name: &str) -> String {
    format!("draft/{name}")
}

/// Show every draft's panel (by its engine name) on the next rebuild; a
/// draft generated without one gets the stock panel.
fn sync_draft_panels(drafts: &[Draft]) {
    set_draft_instrument_uis(
        drafts
            .iter()
            .filter_map(|draft| {
                let ui = draft.instrument.ui.clone()?;
                let path = draft.dir.join("ui.lisp").display().to_string();
                Some((draft_engine_name(&draft.instrument.name), (path, ui)))
            })
            .collect(),
    );
}

fn buffer_key(buffer: &eseqlisp::buffer::Buffer) -> String {
    match &buffer.path {
        Some(path) => path.display().to_string(),
        None => format!("buffer:{}", buffer.name),
    }
}

/// True when `buffer`, whose text is `text`, holds a script C-c C-c may
/// evaluate: eseqlisp the user edits (not a rendered layout buffer or a
/// read-only one) that is not a module (a library: evaluating one
/// re-registers a live module).
pub(crate) fn is_script_buffer(buffer: &eseqlisp::buffer::Buffer, text: &str) -> bool {
    buffer.mode == BufferMode::ESeqLisp
        && buffer.widget_tree.is_none()
        && !buffer.read_only
        && sequencer::agent::forms::declared_module(text).is_none()
}

fn write_draft_files(dir: &Path, instrument: &GeneratedInstrument, script: &str) -> Result<(), String> {
    instrument.write_to(dir)?;
    std::fs::write(dir.join("source.lisp"), script)
        .map_err(|error| format!("Failed to write draft source.lisp: {error}"))
}

/// Handle C-c C-c when the active buffer is a script; false when it is not
/// one, so the caller falls through to the code-editor evaluation. The
/// script is evaluated once: a program becomes the buffer's draft, any other
/// value is reported.
pub(super) fn evaluate_script_buffer(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> bool {
    let buffer = editor.active_buffer();
    let script = buffer.text();
    if !is_script_buffer(buffer, &script) {
        return false;
    }
    let key = buffer_key(buffer);
    let path = buffer
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}.lisp", buffer.name)));
    let value = match sequencer::lisp_host::eval_script(editor.runtime_mut(), path.clone(), &script) {
        Ok(value) => value.unwrap_or(Value::Nil),
        Err(errors) => {
            editor_error(editor, errors.join("; "));
            return true;
        }
    };
    if !sequencer::lisp_host::is_program(&value) {
        let shown: String = editor.runtime().format_value(&value).chars().take(160).collect();
        editor.handle_host_event(HostEvent::Status(format!("Evaluated: {shown}")));
        return true;
    }
    let instrument =
        match sequencer::lisp_host::generate_from_program(editor.runtime_mut(), path, value) {
            Ok(instrument) => instrument,
            Err(errors) => {
                editor_error(editor, format!("Program: {}", errors.join("; ")));
                return true;
            }
        };

    let live = ctx.sessions.drafts.iter().position(|draft| {
        draft.key == key && app.graph.track_engine_ids.contains(&Some(draft.engine_id))
    });
    match live {
        Some(index) => update_draft(app, editor, ctx, index, instrument, &script),
        None => {
            // A draft whose track was deleted: drop it with its files.
            let mut replaced = false;
            ctx.sessions.drafts.retain(|draft| {
                if draft.key != key {
                    return true;
                }
                replaced = true;
                let _ = std::fs::remove_dir_all(&draft.dir);
                false
            });
            sync_draft_panels(&ctx.sessions.drafts);
            create_draft(app, editor, ctx, key, instrument, &script, replaced);
        }
    }
    true
}

fn update_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    index: usize,
    instrument: GeneratedInstrument,
    script: &str,
) {
    let draft = &mut ctx.sessions.drafts[index];
    if let Err(error) = write_draft_files(&draft.dir, &instrument, script) {
        editor_error(editor, error);
        return;
    }
    draft.generation = draft.generation.wrapping_add(1);
    let receiver = spawn_preview_compile(
        PreviewSource::Source {
            compile_source: instrument.dsp.clone(),
            source: instrument.dsp.clone(),
            layout: None,
        },
        app.graph.sample_rate,
        Some(draft.dir.clone()),
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    );
    let key = draft.key.clone();
    let generation = draft.generation;
    let status = format!("Compiling draft '{}'...", instrument.name);
    ctx.sessions.pending_draft_compiles.retain(|pending| pending.key != key);
    ctx.sessions.pending_draft_compiles.push(PendingDraftCompile { key, generation, instrument, receiver });
    editor.handle_host_event(HostEvent::Status(status));
}

fn create_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    key: String,
    instrument: GeneratedInstrument,
    script: &str,
    replaced: bool,
) {
    let dir = match create_new_instrument_draft_dir() {
        Ok(dir) => dir,
        Err(error) => {
            editor_error(editor, error);
            return;
        }
    };
    if let Err(error) = write_draft_files(&dir, &instrument, script) {
        let _ = std::fs::remove_dir_all(&dir);
        editor_error(editor, error);
        return;
    }
    let track = match app.add_transient_instrument_track_sync(
        &draft_engine_name(&instrument.name),
        &instrument.dsp,
        Some(&dir),
    ) {
        Ok(track) => track,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            editor_error(editor, format!("Draft did not compile: {error}"));
            return;
        }
    };
    let _ = app.force_instrument_enabled(track);
    let Some(engine_id) = app.graph.track_engine_ids.get(track).and_then(|id| *id) else {
        let _ = app.graph_controller().delete_track(track);
        let _ = std::fs::remove_dir_all(&dir);
        editor_error(editor, "Draft track has no engine binding");
        return;
    };
    let status = format!(
        "{} draft track {}: {}",
        if replaced { "Its draft was deleted; created" } else { "Created" },
        track + 1,
        instrument.name
    );
    ctx.sessions.drafts.push(Draft { key, engine_id, dir, generation: 0, instrument });
    sync_draft_panels(&ctx.sessions.drafts);
    reload_instrument_panels(editor);
    sync_after_instrument_track_apply(
        app,
        editor,
        &ctx.shared.state,
        track,
        &ctx.shared.current_track,
        &mut *ctx.track_names,
        &ctx.shared.track_pan_ids,
        &ctx.shared.record_armed,
        &ctx.shared.accumulator_names,
        &ctx.shared.ui_epoch,
        ctx.shared.lg_raw,
    );
    editor.handle_host_event(HostEvent::Status(status));
}

/// Apply finished draft recompiles. Knob values carry over by param name
/// (the hot reload keeps them); a param whose default the script changed
/// takes the new default, so editing `:pressure 0.4` in the script is heard.
pub(crate) fn poll_draft_compiles(
    app: &mut app::App,
    editor: &mut Editor,
    sessions: &mut EditSessionState,
    ui_epoch: &AtomicUsize,
) {
    if sessions.pending_draft_compiles.is_empty() {
        return;
    }
    let mut finished = Vec::new();
    for pending in std::mem::take(&mut sessions.pending_draft_compiles) {
        let outcome = match pending.receiver.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                sessions.pending_draft_compiles.push(pending);
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        finished.push((pending.key, pending.generation, pending.instrument, outcome));
    }
    let mut panels_changed = false;
    for (key, generation, instrument, outcome) in finished {
        let Some(draft) = sessions
            .drafts
            .iter_mut()
            .find(|draft| draft.key == key && draft.generation == generation)
        else {
            continue;
        };
        let compiled = match outcome.map(|outcome| outcome.result) {
            Some(Ok(compiled)) => compiled,
            Some(Err(error)) => {
                editor_error(editor, format!("Draft did not compile: {error}"));
                continue;
            }
            None => {
                editor_error(editor, "Draft compile thread crashed");
                continue;
            }
        };
        let Some(track) = app
            .graph
            .track_engine_ids
            .iter()
            .position(|engine| *engine == Some(draft.engine_id))
        else {
            continue;
        };
        let old_defaults: HashMap<String, f32> = app
            .graph
            .instrument_descriptors
            .get(track)
            .map(|desc| desc.params.iter().map(|p| (p.name.clone(), p.default)).collect())
            .unwrap_or_default();
        if let Err(error) = app.apply_compiled_instrument_engine(
            draft.engine_id,
            &draft_engine_name(&instrument.name),
            &compiled.source,
            compiled.result,
        ) {
            editor_error(editor, error);
            continue;
        }
        // The panel dispatches on the engine name, so a rename changes it too.
        panels_changed |= draft.instrument.ui != instrument.ui || draft.instrument.name != instrument.name;
        draft.instrument = instrument;
        let changed: Vec<(usize, f32)> = app
            .graph
            .instrument_descriptors
            .get(track)
            .map(|desc| {
                desc.params
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| old_defaults.get(&p.name).is_some_and(|old| *old != p.default))
                    .map(|(index, p)| (index, p.default))
                    .collect()
            })
            .unwrap_or_default();
        for (param_idx, value) in changed {
            app::apply_command(app, app::AppCommand::SetInstrumentParam { track, param_idx, value });
        }
        ui_epoch.fetch_add(1, Ordering::Relaxed);
        editor.handle_host_event(HostEvent::Status(format!("Updated draft '{}'", draft.instrument.name)));
    }
    if panels_changed {
        sync_draft_panels(&sessions.drafts);
        reload_instrument_panels(editor);
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
    }
}

/// The `:track` of `payload` and the draft playing on it.
fn draft_on_track<'a>(
    app: &app::App,
    drafts: &'a [Draft],
    payload: &Value,
) -> Result<(usize, &'a Draft), String> {
    let track = extract_usize_from_payload(payload, "track").ok_or("Missing draft track")?;
    let engine_id = app
        .graph
        .track_engine_ids
        .get(track)
        .copied()
        .flatten()
        .ok_or("That track has no instrument")?;
    drafts
        .iter()
        .find(|draft| draft.engine_id == engine_id)
        .map(|draft| (track, draft))
        .ok_or_else(|| "That track is not a draft".to_string())
}

/// Write the draft's running instrument into the user instrument folder as
/// `name`. Replacing a saved one needs `overwrite`; the track stays the
/// script's draft, so later evaluations keep recompiling it. An error
/// carries the typed name when that name is already saved (Save becomes
/// Replace), else "".
fn save_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    payload: &Value,
) -> Result<String, (String, String)> {
    let (_, draft) = draft_on_track(app, &ctx.sessions.drafts, payload)
        .map_err(|error| (error, String::new()))?;
    let raw = extract_string_from_payload(payload, "name").unwrap_or_default();
    if raw.trim().is_empty() {
        return Err(("Name the instrument".to_string(), String::new()));
    }
    let slug = sequencer::agent::actions::normalize_patch_name(&raw, "instrument");
    let (dir, legacy_file) = finalized_instrument_storage_paths(&slug);
    let exists = format!("Instrument '{slug}' already exists");
    if legacy_file.exists() {
        // A single-file instrument: not one a draft save can replace.
        return Err((exists, String::new()));
    }
    if dir.exists() && !extract_bool_from_payload(payload, "overwrite") {
        return Err((exists, raw));
    }
    // A replaced patcher-built instrument's layout sidecar would mark the
    // generated dsp as patch-authored.
    let _ = std::fs::remove_file(patcher_layout_sidecar_path_for_dsp(&dir.join("dsp.lisp")));
    // What the running engine was generated as: the draft's own files may
    // hold a recompile that failed or is still pending.
    GeneratedInstrument { name: slug.clone(), ..draft.instrument.clone() }
        .write_to(&dir)
        .map_err(|error| (error, String::new()))?;
    sequencer::lisp_host::invalidate_instrument_release_index();
    // Puts the saved instrument's panel in the catalog, as the instrument
    // editor's saves do.
    reload_instrument_panels(editor);
    sync_sidebar_browser(app, app.ui.cursor_track);
    Ok(slug)
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match name {
        "draft-save-open" => {
            draft_on_track(app, &ctx.sessions.drafts, &payload).and_then(|(track, draft)| {
                let default_name = draft.instrument.name.clone();
                // A modal only receives pointer input through the active tile.
                super::file_menu::activate_dialog_tile(editor);
                editor
                    .runtime_mut()
                    .invoke_global(
                        "eseq.draft-save/open",
                        vec![Value::Number(track as f64), Value::String(default_name)],
                    )
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"))
            })
        }
        "draft-save" => match save_draft(app, editor, ctx, &payload) {
            Ok(slug) => {
                editor.show_toast(format!("Saved instrument '{slug}'"), eseqlisp::ToastKind::Success);
                editor
                    .runtime_mut()
                    .invoke_global("eseq.draft-save/close", Vec::new())
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"))
            }
            // The form shows the error; a transient message only without it.
            Err((error, taken)) => editor
                .runtime_mut()
                .invoke_global(
                    "eseq.draft-save/fail",
                    vec![Value::String(error.clone()), Value::String(taken)],
                )
                .map(|_| ())
                .map_err(|_| error),
        },
        _ => Ok(()),
    };
    if let Err(error) = result {
        editor.show_transient_message(error);
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_file_is_not_a_script() {
        let mut script = eseqlisp::buffer::Buffer::from_text(0, "*scratch*", "");
        script.mode = BufferMode::ESeqLisp;
        assert!(is_script_buffer(&script, "(import eseq.gen)\n42"));
        assert!(is_script_buffer(&script, "(module-ish 1)"));
        assert!(!is_script_buffer(&script, ";; a library\n\n(module eseq.mine)\n(export f)"));
        script.read_only = true;
        assert!(!is_script_buffer(&script, "42"));
        script.read_only = false;
        script.widget_tree = Some(Value::Nil);
        assert!(!is_script_buffer(&script, "42"));
    }
}
