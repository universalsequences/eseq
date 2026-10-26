//! Program scripts drive draft tracks (docs/program-generators-spec.md §2-3).
//! C-c C-c in a script buffer evaluates it; when its last value is a program
//! (an `eseq.gen` program, such as an `eseq.modal` patch) the generated
//! instrument plays on that buffer's own draft track: the first evaluation
//! adds the track, later ones recompile it in place (a file-backed buffer
//! keeps its track by path). The draft is an ordinary track in the normal
//! layout; no editor session takes over the screen. Saving it turns the
//! track into the saved instrument; the buffer's next evaluation starts a
//! new draft.

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

/// Where a script buffer's relative files resolve from: its file, or for an
/// unsaved buffer `<name>.lisp` in the working folder.
pub(super) fn script_path(buffer: &eseqlisp::buffer::Buffer) -> PathBuf {
    buffer.path.clone().unwrap_or_else(|| PathBuf::from(format!("{}.lisp", buffer.name)))
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
    let name = editor.active_buffer().name.clone();
    match evaluate_script_buffer_named(app, editor, ctx, &name) {
        ScriptEvaluation::NotAScript => false,
        ScriptEvaluation::Failed(error) => {
            editor_error(editor, error);
            true
        }
        _ => true,
    }
}

/// What evaluating a script buffer did.
pub(super) enum ScriptEvaluation {
    NotAScript,
    /// A value that is not a program, reported in the status line.
    Value,
    /// A new draft track, ready.
    Created,
    /// The buffer's draft is recompiling (key as in [`Draft::key`]);
    /// [`poll_draft_compiles`] reports when it is done.
    Compiling(String),
    Failed(String),
}

/// [`evaluate_script_buffer`] for the buffer named `name`, active or not
/// (the shape lab evaluates the script it edits). The outcome goes to the
/// caller; a failure is not also put in the status line.
pub(super) fn evaluate_script_buffer_named(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    name: &str,
) -> ScriptEvaluation {
    let Some(buffer) = editor.buffers.iter().find(|buffer| buffer.name == name) else {
        return ScriptEvaluation::NotAScript;
    };
    let script = buffer.text();
    if !is_script_buffer(buffer, &script) {
        return ScriptEvaluation::NotAScript;
    }
    let key = buffer_key(buffer);
    let path = script_path(buffer);
    let value = match sequencer::lisp_host::eval_script(editor.runtime_mut(), path.clone(), &script) {
        Ok(value) => value.unwrap_or(Value::Nil),
        Err(errors) => return ScriptEvaluation::Failed(errors.join("; ")),
    };
    if !sequencer::lisp_host::is_program(&value) {
        let shown: String = editor.runtime().format_value(&value).chars().take(160).collect();
        editor.handle_host_event(HostEvent::Status(format!("Evaluated: {shown}")));
        return ScriptEvaluation::Value;
    }
    let instrument =
        match sequencer::lisp_host::generate_from_program(editor.runtime_mut(), path, value) {
            Ok(instrument) => instrument,
            Err(errors) => return ScriptEvaluation::Failed(format!("Program: {}", errors.join("; "))),
        };

    let live = ctx.sessions.drafts.iter().position(|draft| {
        draft.key == key && app.graph.track_engine_ids.contains(&Some(draft.engine_id))
    });
    let outcome = match live {
        Some(index) => update_draft(app, editor, ctx, index, instrument, &script).map(ScriptEvaluation::Compiling),
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
            create_draft(app, editor, ctx, key, instrument, &script, replaced).map(|()| ScriptEvaluation::Created)
        }
    };
    outcome.unwrap_or_else(ScriptEvaluation::Failed)
}

fn update_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    index: usize,
    instrument: GeneratedInstrument,
    script: &str,
) -> Result<String, String> {
    let draft = &mut ctx.sessions.drafts[index];
    write_draft_files(&draft.dir, &instrument, script)?;
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
    ctx.sessions.pending_draft_compiles.push(PendingDraftCompile { key: key.clone(), generation, instrument, receiver });
    editor.handle_host_event(HostEvent::Status(status));
    Ok(key)
}

fn create_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    key: String,
    instrument: GeneratedInstrument,
    script: &str,
    replaced: bool,
) -> Result<(), String> {
    let dir = create_new_instrument_draft_dir()?;
    if let Err(error) = write_draft_files(&dir, &instrument, script) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error);
    }
    let track = match app.add_transient_instrument_track_sync(
        &draft_engine_name(&instrument.name),
        &instrument.dsp,
        Some(&dir),
    ) {
        Ok(track) => track,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("Draft did not compile: {error}"));
        }
    };
    let _ = app.force_instrument_enabled(track);
    let Some(engine_id) = app.graph.track_engine_ids.get(track).and_then(|id| *id) else {
        let _ = app.graph_controller().delete_track(track);
        let _ = std::fs::remove_dir_all(&dir);
        return Err("Draft track has no engine binding".into());
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
    Ok(())
}

/// Apply finished draft recompiles. Knob values carry over by param name
/// (the hot reload keeps them); a param whose default the script changed
/// takes the new default, so editing `:pressure 0.4` in the script is heard.
/// Each finished draft's key and outcome is returned (also reported in the
/// status line), for a caller waiting on one.
pub(crate) fn poll_draft_compiles(
    app: &mut app::App,
    editor: &mut Editor,
    sessions: &mut EditSessionState,
    ui_epoch: &AtomicUsize,
) -> Vec<(String, Result<(), String>)> {
    let mut outcomes = Vec::new();
    if sessions.pending_draft_compiles.is_empty() {
        return outcomes;
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
                let error = format!("Draft did not compile: {error}");
                editor_error(editor, error.clone());
                outcomes.push((key, Err(error)));
                continue;
            }
            None => {
                editor_error(editor, "Draft compile thread crashed");
                outcomes.push((key, Err("Draft compile thread crashed".into())));
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
            editor_error(editor, error.clone());
            outcomes.push((key, Err(error)));
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
        outcomes.push((key, Ok(())));
    }
    if panels_changed {
        sync_draft_panels(&sessions.drafts);
        reload_instrument_panels(editor);
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
    }
    outcomes
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
/// `name`, then make the track play that saved instrument: its engine takes
/// the saved id (what a project save writes), so the knob and modulation
/// settings on it survive a project round trip. The track stops being a
/// draft; the script's next evaluation starts a new one. Replacing a saved
/// instrument needs `overwrite`. An error carries the typed name when that
/// name is already saved (Save becomes Replace), else "".
fn save_draft(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    payload: &Value,
) -> Result<String, (String, String)> {
    let (_, draft) = draft_on_track(app, &ctx.sessions.drafts, payload)
        .map_err(|error| (error, String::new()))?;
    let engine_id = draft.engine_id;
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
    adopt_saved_instrument(app, ctx.sessions, ctx.track_names, engine_id, &slug);
    ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    // Puts the saved instrument's panel in the catalog, as the instrument
    // editor's saves do; the track's panel is now that one.
    sync_draft_panels(&ctx.sessions.drafts);
    reload_instrument_panels(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");
    sync_sidebar_browser(app, app.ui.cursor_track);
    Ok(slug)
}

/// Turn draft engine `engine_id` into the user instrument `slug` just saved:
/// drop the draft (and any recompile still pending for it) and rename the
/// engine to the saved id, keeping its compiled state and every track's
/// values. Tracks whose name was the draft's automatic one take the new name.
fn adopt_saved_instrument(
    app: &mut app::App,
    sessions: &mut EditSessionState,
    track_names: &mut Vec<String>,
    engine_id: usize,
    slug: &str,
) {
    let Some(index) = sessions.drafts.iter().position(|draft| draft.engine_id == engine_id) else {
        return;
    };
    let draft = sessions.drafts.remove(index);
    sessions.pending_draft_compiles.retain(|pending| pending.key != draft.key);
    // The draft's directory stays: it is the running engine's asset base.
    let user_id = sequencer::app_paths::ContentTier::User.qualify(slug);
    let id = sequencer::lisp_host::qualify_instrument_id(&user_id).unwrap_or(user_id);
    app.rename_instrument_engine(engine_id, &id);
    for track in 0..app.tracks.len() {
        if app.graph.track_engine_ids.get(track).copied().flatten() == Some(engine_id) {
            app.set_automatic_track_name(track, slug.to_string());
        }
    }
    super::refresh_track_names_cache(track_names, app);
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

    /// After Save the track plays the saved instrument: its engine answers to
    /// the saved user id (what a project save writes), the draft is gone, and
    /// the automatic track name follows the typed name.
    #[test]
    fn a_saved_draft_becomes_the_saved_instrument() {
        let state = Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = app::App::new(
            state,
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        let manifest = sequencer::lisp_host::parse_manifest(
            r#"{"processAbi": "dgen-host-abi-v1", "outputs": [{"channel": 0}]}"#,
        )
        .expect("manifest parses");
        let engine_id = app.editor.engine_registry.upsert(app::EngineDescriptor {
            name: draft_engine_name("modal-eno-lead"),
            source: "(out 0)".to_string(),
            manifest,
            lib_index: 0,
            shared_runtime: true,
        });
        app.tracks = vec!["modal-eno-lead".to_string()];
        app.graph.track_engine_ids = vec![Some(engine_id)];
        let mut sessions = EditSessionState::default();
        sessions.drafts.push(Draft {
            key: "/scripts/modal-eno-lead.lisp".to_string(),
            engine_id,
            dir: std::env::temp_dir().join("eseq-draft-adopt-test-missing"),
            generation: 0,
            instrument: GeneratedInstrument {
                name: "modal-eno-lead".to_string(),
                dsp: "(out 0)".to_string(),
                ui: None,
                voice_mode: None,
                assets: Vec::new(),
            },
        });
        let mut track_names = app.tracks.clone();

        let slug = "eno-lead-draft-adopt-test";
        adopt_saved_instrument(&mut app, &mut sessions, &mut track_names, engine_id, slug);

        let engine = app.editor.engine_registry.get(engine_id).expect("engine kept");
        assert!(
            engine.name.starts_with(&sequencer::app_paths::ContentTier::User.qualify(slug)),
            "{}",
            engine.name
        );
        assert!(sessions.drafts.is_empty());
        assert_eq!(app.tracks, vec![slug.to_string()]);
        assert_eq!(track_names, app.tracks);
    }
}
