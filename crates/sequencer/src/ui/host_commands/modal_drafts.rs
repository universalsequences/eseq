//! Modal patch scripts drive draft tracks (eseq-k0m2.9). C-c C-c in a buffer
//! that imports `eseq.modal` evaluates it, generates the instrument, and plays
//! it on that buffer's own draft track: the first evaluation adds the track,
//! later ones recompile it in place (a file-backed buffer keeps its track by
//! path). The draft is an ordinary track in the normal layout; no editor
//! session takes over the screen.

use crate::*;

/// A draft track one script buffer drives.
pub(crate) struct ModalDraft {
    /// The buffer's file path, or `buffer:<name>` for an unsaved buffer.
    pub(crate) key: String,
    pub(crate) engine_id: usize,
    /// The patch's :name; the engine runs as `modal-draft/<name>`.
    pub(crate) name: String,
    /// Holds the generated dsp.lisp / ui.lisp / instrument.json and the
    /// script (modal.lisp), and is the draft's asset base.
    pub(crate) dir: PathBuf,
    pub(crate) generation: u64,
    /// The ui.lisp source of the engine running now: the panel shows this,
    /// not the file, which a pending compile has already overwritten.
    pub(crate) ui: String,
}

pub(crate) struct PendingModalCompile {
    key: String,
    generation: u64,
    /// The patch's :name and panel, recorded on the draft once the compile
    /// applies.
    name: String,
    ui: String,
    receiver: std::sync::mpsc::Receiver<PreviewCompileOutcome>,
}

fn draft_engine_name(name: &str) -> String {
    format!("modal-draft/{name}")
}

/// Show every draft's panel (by its engine name) on the next rebuild.
fn sync_draft_panels(drafts: &[ModalDraft]) {
    set_draft_instrument_uis(
        drafts
            .iter()
            .map(|draft| {
                let path = draft.dir.join("ui.lisp").display().to_string();
                (draft_engine_name(&draft.name), (path, draft.ui.clone()))
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

fn write_draft_files(
    dir: &Path,
    instrument: &sequencer::lisp_host::ModalInstrument,
    script: &str,
) -> Result<(), String> {
    instrument.write_to(dir)?;
    std::fs::write(dir.join("modal.lisp"), script)
        .map_err(|error| format!("Failed to write draft modal.lisp: {error}"))
}

/// Handle C-c C-c when the active buffer is a modal patch script; false when
/// it is not one, so the caller falls through to the code-editor evaluation.
pub(super) fn evaluate_modal_buffer(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> bool {
    let buffer = editor.active_buffer();
    let script = buffer.text();
    if !sequencer::lisp_host::is_modal_patch_script(&script) {
        return false;
    }
    let key = buffer_key(buffer);
    let path = buffer
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}.lisp", buffer.name)));
    let instrument =
        match sequencer::lisp_host::generate_modal_instrument(editor.runtime_mut(), path, &script) {
            Ok(instrument) => instrument,
            Err(errors) => {
                editor_error(editor, format!("Modal patch: {}", errors.join("; ")));
                return true;
            }
        };

    let live = ctx.sessions.modal_drafts.iter().position(|draft| {
        draft.key == key && app.graph.track_engine_ids.contains(&Some(draft.engine_id))
    });
    match live {
        Some(index) => update_draft(app, editor, ctx, index, instrument, &script),
        None => {
            // A draft whose track was deleted: drop it with its files.
            let mut replaced = false;
            ctx.sessions.modal_drafts.retain(|draft| {
                if draft.key != key {
                    return true;
                }
                replaced = true;
                let _ = std::fs::remove_dir_all(&draft.dir);
                false
            });
            sync_draft_panels(&ctx.sessions.modal_drafts);
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
    instrument: sequencer::lisp_host::ModalInstrument,
    script: &str,
) {
    let draft = &mut ctx.sessions.modal_drafts[index];
    if let Err(error) = write_draft_files(&draft.dir, &instrument, script) {
        editor_error(editor, error);
        return;
    }
    draft.generation = draft.generation.wrapping_add(1);
    let receiver = spawn_preview_compile(
        PreviewSource::Source {
            compile_source: instrument.dsp.clone(),
            source: instrument.dsp,
            layout: None,
        },
        app.graph.sample_rate,
        Some(draft.dir.clone()),
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    );
    let key = draft.key.clone();
    let generation = draft.generation;
    let status = format!("Compiling draft '{}'...", instrument.name);
    ctx.sessions.pending_modal_compiles.retain(|pending| pending.key != key);
    ctx.sessions.pending_modal_compiles.push(PendingModalCompile {
        key,
        generation,
        name: instrument.name,
        ui: instrument.ui,
        receiver,
    });
    editor.handle_host_event(HostEvent::Status(status));
}

fn create_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    key: String,
    instrument: sequencer::lisp_host::ModalInstrument,
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
            editor_error(editor, format!("Modal patch did not compile: {error}"));
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
    ctx.sessions.modal_drafts.push(ModalDraft {
        key,
        engine_id,
        name: instrument.name,
        dir,
        generation: 0,
        ui: instrument.ui,
    });
    sync_draft_panels(&ctx.sessions.modal_drafts);
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
pub(crate) fn poll_modal_draft_compiles(
    app: &mut app::App,
    editor: &mut Editor,
    sessions: &mut EditSessionState,
    ui_epoch: &AtomicUsize,
) {
    let mut finished = Vec::new();
    sessions.pending_modal_compiles.retain(|pending| match pending.receiver.try_recv() {
        Ok(outcome) => {
            finished.push((pending.key.clone(), pending.generation, pending.name.clone(), pending.ui.clone(), Some(outcome)));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            finished.push((pending.key.clone(), pending.generation, pending.name.clone(), pending.ui.clone(), None));
            false
        }
    });
    let mut panels_changed = false;
    for (key, generation, name, ui, outcome) in finished {
        let Some(draft) = sessions
            .modal_drafts
            .iter_mut()
            .find(|draft| draft.key == key && draft.generation == generation)
        else {
            continue;
        };
        let compiled = match outcome.map(|outcome| outcome.result) {
            Some(Ok(compiled)) => compiled,
            Some(Err(error)) => {
                editor_error(editor, format!("Modal patch did not compile: {error}"));
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
            &draft_engine_name(&name),
            &compiled.source,
            compiled.result,
        ) {
            editor_error(editor, error);
            continue;
        }
        // The panel dispatches on the engine name, so a rename changes it too.
        panels_changed |= draft.ui != ui || draft.name != name;
        draft.ui = ui;
        draft.name = name;
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
        editor.handle_host_event(HostEvent::Status(format!("Updated draft '{}'", draft.name)));
    }
    if panels_changed {
        sync_draft_panels(&sessions.modal_drafts);
        reload_instrument_panels(editor);
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
    }
}
