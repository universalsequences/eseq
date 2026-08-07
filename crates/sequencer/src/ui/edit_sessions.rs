use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FxDeleteChain {
    Audio,
    Midi,
    Bus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActiveDeleteTarget {
    MixerTrack {
        track: usize,
    },
    TrackPattern {
        track: usize,
        pattern_id: PatternId,
    },
    ModRoute {
        source: usize,
        destination: sequencer::sequencer::ModDestination,
        input: usize,
    },
    FxEffect {
        chain: FxDeleteChain,
        bus: Option<usize>,
        slot: usize,
    },
    RackEffect {
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
    },
    RackSlot {
        track: usize,
        slot: usize,
    },
}

/// Which editing surface a dsp edit session uses
/// (docs/patch-vs-code-editor-spec.md §3.2): the patch editor for
/// patch-authored content and new drafts, a plain DGenLisp text buffer for
/// everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorSurface {
    Patch,
    Code,
}

pub(super) fn editor_surface_for_existing(
    path: &Path,
    source: &str,
    intent: eseqlisp::widget_render::patcher::PatcherIntent,
) -> EditorSurface {
    if eseqlisp::widget_render::patcher::source_opens_in_patch_editor(path, source, intent) {
        EditorSurface::Patch
    } else {
        EditorSurface::Code
    }
}

pub(super) fn editor_surface_label(surface: EditorSurface) -> &'static str {
    match surface {
        EditorSurface::Patch => "patch",
        EditorSurface::Code => "code",
    }
}

#[derive(Debug, Clone)]
pub(super) enum InstrumentEditMode {
    EditExisting {
        persisted_source: String,
    },
    CreateDraft {
        temp_dir: PathBuf,
        draft_track: usize,
        original_track: usize,
    },
}

#[derive(Debug, Clone)]
pub(super) struct InstrumentEditSession {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) buffer_name: String,
    pub(super) engine_id: usize,
    pub(super) track: usize,
    pub(super) last_valid_source: String,
    pub(super) last_valid_layout: Option<String>,
    pub(super) visible_revision_valid: bool,
    pub(super) preview_generation: u64,
    pub(super) run_mode: CustomInstrumentRunMode,
    pub(super) surface: EditorSurface,
    pub(super) mode: InstrumentEditMode,
}

impl InstrumentEditSession {
    pub(super) fn begin_edit_existing(
        name: String,
        path: PathBuf,
        buffer_name: String,
        engine_id: usize,
        track: usize,
        persisted_source: String,
        run_mode: CustomInstrumentRunMode,
        surface: EditorSurface,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            engine_id,
            track,
            last_valid_source: persisted_source.clone(),
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            run_mode,
            surface,
            mode: InstrumentEditMode::EditExisting { persisted_source },
        }
    }

    pub(super) fn begin_create_draft(
        name: String,
        path: PathBuf,
        buffer_name: String,
        engine_id: usize,
        source: String,
        temp_dir: PathBuf,
        draft_track: usize,
        original_track: usize,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            engine_id,
            track: draft_track,
            last_valid_source: source,
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            run_mode: CustomInstrumentRunMode::Instrument,
            surface: EditorSurface::Patch,
            mode: InstrumentEditMode::CreateDraft {
                temp_dir,
                draft_track,
                original_track,
            },
        }
    }
}

pub(super) struct PendingInstrumentPreview {
    pub(super) generation: u64,
    pub(super) source: String,
    pub(super) layout: Option<String>,
    pub(super) receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_host::CompileResult, String>>,
}

pub(super) struct PendingInstrumentCancelRestore {
    pub(super) session: InstrumentEditSession,
    pub(super) persisted_source: String,
    pub(super) receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_host::CompileResult, String>>,
}

pub(super) struct PendingSavedInstrumentLoad {
    pub(super) name: String,
    pub(super) source: String,
    pub(super) run_mode: CustomInstrumentRunMode,
    pub(super) target: SavedInstrumentLoadTarget,
    pub(super) receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_host::CompileResult, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SavedInstrumentLoadTarget {
    AddTrack {
        group_id: Option<u64>,
    },
    SwapTrack {
        track_id: TrackId,
    },
}

pub(super) enum SavedInstrumentLoadApply {
    Added { track: usize, group_id: Option<u64> },
    Swapped { summary: InstrumentSlotResetSummary },
}

pub(super) fn capture_instrument_swap_target(
    app: &app::App,
    track: usize,
) -> Result<SavedInstrumentLoadTarget, String> {
    match app.graph.track_instrument_types.get(track) {
        Some(InstrumentType::Custom) => {
            if app
                .graph
                .track_engine_ids
                .get(track)
                .and_then(|engine_id| *engine_id)
                .is_none()
            {
                return Err(format!("Custom track {} has no engine binding", track + 1));
            }
        }
        Some(InstrumentType::Sampler) => {}
        Some(InstrumentType::Rack) => {}
        Some(other) => {
            return Err(format!(
                "Track {} has instrument type {other:?}, which cannot be replaced",
                track + 1
            ));
        }
        None => return Err(format!("Track {} does not exist", track + 1)),
    }
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
    Ok(SavedInstrumentLoadTarget::SwapTrack {
        track_id,
    })
}

pub(super) fn resolve_instrument_swap_target(
    app: &app::App,
    track_id: TrackId,
) -> Result<usize, String> {
    app.track_registry
        .index_of(track_id)
        .ok_or_else(|| "The instrument swap target was removed while loading".to_string())
}

pub(super) fn try_apply_cached_saved_instrument(
    app: &mut app::App,
    target: SavedInstrumentLoadTarget,
    name: &str,
    source: &str,
    run_mode: CustomInstrumentRunMode,
) -> Option<Result<SavedInstrumentLoadApply, String>> {
    match target {
        SavedInstrumentLoadTarget::AddTrack { group_id } => app
            .try_add_cached_saved_instrument_track_sync(name, source, run_mode)
            .map(|result| result.map(|track| SavedInstrumentLoadApply::Added { track, group_id })),
        SavedInstrumentLoadTarget::SwapTrack {
            track_id,
        } => {
            let track = match resolve_instrument_swap_target(app, track_id) {
                Ok(track) => track,
                Err(error) => return Some(Err(error)),
            };
            app.try_swap_track_to_cached_saved_instrument_sync(track, name, source, run_mode)
                .map(|result| result.map(|summary| SavedInstrumentLoadApply::Swapped { summary }))
        }
    }
}

pub(super) fn apply_compiled_saved_instrument(
    app: &mut app::App,
    target: SavedInstrumentLoadTarget,
    name: &str,
    source: &str,
    run_mode: CustomInstrumentRunMode,
    result: sequencer::lisp_host::CompileResult,
) -> Result<SavedInstrumentLoadApply, String> {
    match target {
        SavedInstrumentLoadTarget::AddTrack { group_id } => app
            .add_compiled_saved_instrument_track_sync(name, source, run_mode, result)
            .map(|track| SavedInstrumentLoadApply::Added { track, group_id }),
        SavedInstrumentLoadTarget::SwapTrack {
            track_id,
        } => {
            let track = resolve_instrument_swap_target(app, track_id)?;
            app.swap_track_to_compiled_saved_instrument_sync(track, name, source, run_mode, result)
                .map(|summary| SavedInstrumentLoadApply::Swapped { summary })
        }
    }
}

pub(super) struct PendingKeyLockAudition {
    pub(super) track: usize,
    pub(super) transpose: f32,
    pub(super) release_at: Instant,
}

pub(super) const KEY_LOCK_AUDITION_DURATION: Duration = Duration::from_millis(220);

pub(super) fn send_keyboard_note_off(
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    track: usize,
    transpose: f32,
) {
    let _ = keyboard_tx.send(KeyboardTrigger {
        track,
        transpose,
        velocity: 0.0,
        note_off: true,
    });
}

pub(super) fn release_due_key_lock_auditions(
    pending: &mut Vec<PendingKeyLockAudition>,
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    now: Instant,
) {
    let mut idx = 0;
    while idx < pending.len() {
        if pending[idx].release_at <= now {
            let audition = pending.swap_remove(idx);
            send_keyboard_note_off(keyboard_tx, audition.track, audition.transpose);
        } else {
            idx += 1;
        }
    }
}

pub(super) fn release_matching_key_lock_auditions(
    pending: &mut Vec<PendingKeyLockAudition>,
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    track: usize,
    transpose: f32,
) {
    let mut idx = 0;
    while idx < pending.len() {
        let audition = &pending[idx];
        if audition.track == track && (audition.transpose - transpose).abs() < 0.01 {
            let audition = pending.swap_remove(idx);
            send_keyboard_note_off(keyboard_tx, audition.track, audition.transpose);
        } else {
            idx += 1;
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum EffectEditTarget {
    Track { track: usize, slot: usize },
    Bus { bus: usize, slot: usize },
}

#[derive(Debug, Clone)]
pub(super) enum EffectEditMode {
    EditExisting { persisted_source: String },
    CreateDraft { temp_dir: PathBuf },
}

#[derive(Debug, Clone)]
pub(super) struct EffectEditSession {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) buffer_name: String,
    pub(super) target: EffectEditTarget,
    pub(super) last_valid_source: String,
    pub(super) last_valid_layout: Option<String>,
    pub(super) visible_revision_valid: bool,
    pub(super) preview_generation: u64,
    pub(super) surface: EditorSurface,
    pub(super) mode: EffectEditMode,
}

impl EffectEditSession {
    pub(super) fn begin_edit_existing(
        name: String,
        path: PathBuf,
        buffer_name: String,
        target: EffectEditTarget,
        persisted_source: String,
        surface: EditorSurface,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            target,
            last_valid_source: persisted_source.clone(),
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            surface,
            mode: EffectEditMode::EditExisting { persisted_source },
        }
    }

    pub(super) fn begin_create_draft(
        name: String,
        path: PathBuf,
        buffer_name: String,
        target: EffectEditTarget,
        source: String,
        temp_dir: PathBuf,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            target,
            last_valid_source: source,
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            surface: EditorSurface::Patch,
            mode: EffectEditMode::CreateDraft { temp_dir },
        }
    }
}

pub(super) struct PendingEffectPreview {
    pub(super) generation: u64,
    pub(super) source: String,
    pub(super) layout: Option<String>,
    pub(super) receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_host::CompileResult, String>>,
}

pub(super) fn editor_macro_action_strings(
    action: Option<&eseqlisp::widget_render::patcher::ActiveMacroLibraryAction>,
) -> (String, String) {
    let Some(action) = action else {
        return (String::new(), String::new());
    };
    let action_name = match action.kind {
        eseqlisp::widget_render::patcher::MacroLibraryActionKind::SaveToLibrary => {
            "save-to-library"
        }
        eseqlisp::widget_render::patcher::MacroLibraryActionKind::Fork => "fork",
    };
    (action.macro_name.clone(), action_name.to_string())
}

pub(super) fn active_instrument_editor_macro_action(
    session: &InstrumentEditSession,
) -> Option<eseqlisp::widget_render::patcher::ActiveMacroLibraryAction> {
    if !session.visible_revision_valid {
        return None;
    }
    match eseqlisp::widget_render::patcher::active_macro_library_action_for_path(
        &session.path,
        &session.last_valid_source,
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    ) {
        Ok(action) => action,
        Err(error) => {
            eprintln!(
                "[editor macro-library-action query failed] path={} intent=instrument error={error}",
                session.path.display()
            );
            None
        }
    }
}

pub(super) fn active_effect_editor_macro_action(
    session: &EffectEditSession,
) -> Option<eseqlisp::widget_render::patcher::ActiveMacroLibraryAction> {
    if !session.visible_revision_valid {
        return None;
    }
    match eseqlisp::widget_render::patcher::active_macro_library_action_for_path(
        &session.path,
        &session.last_valid_source,
        eseqlisp::widget_render::patcher::PatcherIntent::Effect,
    ) {
        Ok(action) => action,
        Err(error) => {
            eprintln!(
                "[editor macro-library-action query failed] path={} intent=effect error={error}",
                session.path.display()
            );
            None
        }
    }
}

pub(super) fn apply_active_instrument_editor_macro_action(
    session: &mut InstrumentEditSession,
) -> Result<Option<eseqlisp::widget_render::patcher::MacroLibraryActionResult>, String> {
    if !session.visible_revision_valid {
        return Err("Cannot save macro: the current patch has errors".to_string());
    }
    if active_instrument_editor_macro_action(session).is_none() {
        return Ok(None);
    }
    let result = eseqlisp::widget_render::patcher::apply_active_macro_library_action_for_path(
        &session.path,
        &session.last_valid_source,
        session.last_valid_layout.as_deref(),
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    )?;
    std::fs::write(&session.path, &result.source)
        .map_err(|error| format!("Failed to save patch source after macro action: {error}"))?;
    write_patcher_layout_sidecar(&session.path, &result.layout)
        .map_err(|error| format!("Failed to save patch layout after macro action: {error}"))?;
    session.last_valid_source = result.source.clone();
    session.last_valid_layout = Some(result.layout.clone());
    eseqlisp::widget_render::patcher::reload_patcher_macro_view_for_path(
        &session.path,
        result.macro_name.clone(),
    );
    Ok(Some(result))
}

pub(super) fn apply_active_effect_editor_macro_action(
    session: &mut EffectEditSession,
) -> Result<Option<eseqlisp::widget_render::patcher::MacroLibraryActionResult>, String> {
    if !session.visible_revision_valid {
        return Err("Cannot save macro: the current patch has errors".to_string());
    }
    if active_effect_editor_macro_action(session).is_none() {
        return Ok(None);
    }
    let result = eseqlisp::widget_render::patcher::apply_active_macro_library_action_for_path(
        &session.path,
        &session.last_valid_source,
        session.last_valid_layout.as_deref(),
        eseqlisp::widget_render::patcher::PatcherIntent::Effect,
    )?;
    std::fs::write(&session.path, &result.source)
        .map_err(|error| format!("Failed to save patch source after macro action: {error}"))?;
    write_patcher_layout_sidecar(&session.path, &result.layout)
        .map_err(|error| format!("Failed to save patch layout after macro action: {error}"))?;
    session.last_valid_source = result.source.clone();
    session.last_valid_layout = Some(result.layout.clone());
    eseqlisp::widget_render::patcher::reload_patcher_macro_view_for_path(
        &session.path,
        result.macro_name.clone(),
    );
    Ok(Some(result))
}

pub(super) fn macro_library_action_status(
    result: &eseqlisp::widget_render::patcher::MacroLibraryActionResult,
) -> String {
    match result.kind {
        eseqlisp::widget_render::patcher::MacroLibraryActionKind::SaveToLibrary => {
            format!("Saved macro '{}' to library", result.macro_name)
        }
        eseqlisp::widget_render::patcher::MacroLibraryActionKind::Fork => {
            format!("Forked macro '{}' into current patch", result.macro_name)
        }
    }
}

pub(super) fn flush_staged_instrument_library_macro_edits(
    session: &InstrumentEditSession,
) -> Result<Vec<String>, String> {
    eseqlisp::widget_render::patcher::flush_staged_library_macro_edits_for_path(
        &session.path,
        &session.last_valid_source,
        session.last_valid_layout.as_deref(),
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    )
}

pub(super) fn flush_staged_effect_library_macro_edits(
    session: &EffectEditSession,
) -> Result<Vec<String>, String> {
    eseqlisp::widget_render::patcher::flush_staged_library_macro_edits_for_path(
        &session.path,
        &session.last_valid_source,
        session.last_valid_layout.as_deref(),
        eseqlisp::widget_render::patcher::PatcherIntent::Effect,
    )
}

pub(super) fn staged_library_macro_flush_status(macros: &[String]) -> Option<String> {
    if macros.is_empty() {
        None
    } else {
        Some(format!("Saved library macro edits: {}", macros.join(", ")))
    }
}

pub(super) struct PendingEffectCancelRestore {
    pub(super) session: EffectEditSession,
    pub(super) receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_host::CompileResult, String>>,
}

pub(super) struct PendingAgenticBubble {
    pub(super) path: PathBuf,
    pub(super) intent: eseqlisp::widget_render::patcher::PatcherIntent,
    pub(super) bubble_id: String,
    pub(super) generation: u64,
    pub(super) receiver: std::sync::mpsc::Receiver<
        Result<sequencer::agent::agentic_bubble::AgenticBubbleOutput, String>,
    >,
}

/// Macro sidebar facts scanned from a patch source: top-level
/// `(defmacro name (params...) body...)` forms (with the head symbols each
/// body calls, for nesting) and `(use-defmacro name)` library imports.
pub(super) struct PatchMacroScan {
    /// (name, params, called head symbols in definition order, deduped)
    pub(super) locals: Vec<(String, Vec<String>, Vec<String>)>,
    pub(super) imports: Vec<String>,
}

fn collect_call_head_symbols(expr: &Expression, seen: &mut Vec<String>) {
    let Expression::List(items) = expr else {
        return;
    };
    if let Some(Expression::Symbol(head)) = items.first() {
        if !seen.contains(head) {
            seen.push(head.clone());
        }
    }
    for item in items {
        collect_call_head_symbols(item, seen);
    }
}

pub(super) fn scan_patch_macro_source(source: &str) -> PatchMacroScan {
    let mut scan = PatchMacroScan {
        locals: Vec::new(),
        imports: Vec::new(),
    };
    let Ok(tokens) = Parser::new(source.to_string()).parse() else {
        return scan;
    };
    let Ok(exprs) = ASTParser::new(tokens).parse() else {
        return scan;
    };
    for expr in &exprs {
        let Expression::List(items) = expr else {
            continue;
        };
        match items.as_slice() {
            [
                Expression::Symbol(head),
                Expression::Symbol(name),
                Expression::List(params),
                body @ ..,
            ] if head == "defmacro" => {
                let params = params
                    .iter()
                    .filter_map(|param| match param {
                        Expression::Symbol(param) => Some(param.clone()),
                        _ => None,
                    })
                    .collect();
                let mut calls = Vec::new();
                for expr in body {
                    collect_call_head_symbols(expr, &mut calls);
                }
                calls.retain(|call| call != name);
                scan.locals.push((name.clone(), params, calls));
            }
            [Expression::Symbol(head), Expression::Symbol(name)] if head == "use-defmacro" => {
                scan.imports.push(name.clone());
            }
            _ => {}
        }
    }
    scan
}

pub(super) fn build_patch_macro_sidebar_value(
    entries: &[(String, Vec<String>, Vec<String>)],
) -> Value {
    Value::List(
        entries
            .iter()
            .map(|(name, params, calls)| {
                std::rc::Rc::new(std::cell::RefCell::new(values::map_value(vec![
                    ("name", Value::String(name.clone())),
                    ("params", values::build_string_list(params)),
                    ("calls", values::build_string_list(calls)),
                ])))
            })
            .collect(),
    )
}

pub(super) fn build_library_macro_sidebar_value(
    entries: &[eseqlisp::widget_render::patcher::MacroLibrarySidebarEntry],
    used: &[String],
) -> Value {
    Value::List(
        entries
            .iter()
            .map(|(name, params, outputs, summary, imports)| {
                std::rc::Rc::new(std::cell::RefCell::new(values::map_value(vec![
                    ("name", Value::String(name.clone())),
                    ("params", values::build_string_list(params)),
                    ("outputs", values::build_string_list(outputs)),
                    (
                        "summary",
                        Value::String(summary.clone().unwrap_or_default()),
                    ),
                    ("calls", values::build_string_list(imports)),
                    ("used", Value::Bool(used.contains(name))),
                ])))
            })
            .collect(),
    )
}

pub(super) fn extract_macro_name_from_defmacro(source: &str) -> Option<String> {
    let tokens = Parser::new(source.to_string()).parse().ok()?;
    let exprs = ASTParser::new(tokens).parse().ok()?;
    let Expression::List(items) = exprs.first()? else {
        return None;
    };
    match items.as_slice() {
        [Expression::Symbol(head), Expression::Symbol(name), ..] if head == "defmacro" => {
            Some(name.clone())
        }
        _ => None,
    }
}

pub(super) fn instrument_patcher_buffer_source(buffer_name: &str, path: &Path) -> String {
    let buffer_name = escape_lisp_string(buffer_name);
    let path = escape_lisp_string(&path.to_string_lossy());
    format!(
        "(effect-buffer \"{buffer_name}\"\n  (patcher\n    :intent :instrument\n    :width :fill\n    :height :fill\n    :path \"{path}\"\n    :on-change (lambda (event)\n      (host-command \"preview-instrument-patch\" event))))\n"
    )
}

pub(super) fn instrument_code_buffer_name(name: &str) -> String {
    format!("*instrument-code:{name}*")
}

pub(super) fn effect_code_buffer_name(name: &str) -> String {
    format!("*effect-code:{name}*")
}

pub(super) fn effect_patcher_buffer_source(buffer_name: &str, path: &Path) -> String {
    let buffer_name = escape_lisp_string(buffer_name);
    let path = escape_lisp_string(&path.to_string_lossy());
    format!(
        "(effect-buffer \"{buffer_name}\"\n  (patcher\n    :intent :effect\n    :width :fill\n    :height :fill\n    :path \"{path}\"\n    :on-change (lambda (event)\n      (host-command \"preview-effect-patch\" event))))\n"
    )
}

pub(super) const NEW_INSTRUMENT_DRAFT_NAME: &str = "new-instrument-draft/";
pub(super) const NEW_EFFECT_DRAFT_NAME: &str = "new-effect-draft/";
pub(super) const NEW_SCRIPT_TAB_LABEL: &str = "New Script";
pub(super) const NEW_SCRIPT_TEMPLATE: &str = r#"; ESeqLisp script
; Source-only scripts can still appear as sequencer tabs.
(seq-register-script-source-tab "Untitled Script")

"#;

#[derive(Debug, Clone)]
pub(super) struct ScriptDraftSession {
    pub(super) temp_path: PathBuf,
    pub(super) buffer_name: String,
}

pub(super) const NEW_INSTRUMENT_STARTER_DSP: &str = r#"(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param attack @group amp @env amp-env @role attack @default 5 @min 0 @max 1000 @unit ms)
(param decay @group amp @env amp-env @role decay @default 120 @min 1 @max 2000 @unit ms)
(param sustain @group amp @env amp-env @role sustain @default 0.8 @min 0 @max 1)
(param release @group amp @env amp-env @role release @default 180 @min 1 @max 5000 @unit ms)
(param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

(def env (adsr gate trigger attack decay sustain release))
(def phase (phasor pitch))
(out (* phase env velocity (mod gain)) 1 @name audio)
"#;

pub(super) fn create_new_instrument_draft_dir() -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let dir = std::env::temp_dir()
        .join("eseq-instrument-drafts")
        .join(format!("draft-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create draft instrument directory: {error}"))?;
    Ok(dir)
}

pub(super) fn create_new_effect_draft_dir() -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let dir = std::env::temp_dir()
        .join("eseq-effect-drafts")
        .join(format!("draft-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create draft effect directory: {error}"))?;
    Ok(dir)
}

pub(super) fn create_new_script_draft_path() -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let dir = std::env::temp_dir()
        .join("eseq-script-drafts")
        .join(format!("draft-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create draft script directory: {error}"))?;
    Ok(dir.join("untitled.lisp"))
}

pub(super) fn instrument_run_mode_label(run_mode: CustomInstrumentRunMode) -> &'static str {
    match run_mode {
        CustomInstrumentRunMode::Instrument => "instrument",
        CustomInstrumentRunMode::FreePatch => "free_patch",
    }
}

pub(super) fn instrument_run_mode_from_label(label: &str) -> Option<CustomInstrumentRunMode> {
    CustomInstrumentRunMode::parse(label)
}

pub(super) fn show_instrument_patcher_layout_source(buffer_name: &str) -> String {
    let buffer_name = escape_lisp_string(buffer_name);
    format!("(seq-apply-instrument-patcher-layout \"{buffer_name}\")")
}

pub(super) fn show_instrument_patcher_source_layout_source(
    patcher_buffer_name: &str,
    source_buffer_name: &str,
) -> String {
    let patcher_buffer_name = escape_lisp_string(patcher_buffer_name);
    let source_buffer_name = escape_lisp_string(source_buffer_name);
    format!(
        "(seq-apply-instrument-patcher-source-layout \"{patcher_buffer_name}\" \"{source_buffer_name}\")"
    )
}

pub(super) fn restore_instrument_patcher_layout_source() -> &'static str {
    "(seq-restore-instrument-patcher-layout)"
}

pub(super) fn reset_instrument_patcher_state(path: &Path) {
    eseqlisp::widget_render::patcher::reset_patcher_state_for_path(
        path,
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    );
}

pub(super) fn reset_effect_patcher_state(path: &Path) {
    eseqlisp::widget_render::patcher::reset_patcher_state_for_path(
        path,
        eseqlisp::widget_render::patcher::PatcherIntent::Effect,
    );
}

pub(super) fn escape_lisp_string(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}

pub(super) fn script_source_buffer_name(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/").replace('/', ":");
    format!("*script:{key}*")
}

pub(super) fn script_file_name_from_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let stem = trimmed.trim_end_matches(".lisp");
    let mut normalized = String::new();
    let mut previous_dash = false;
    for ch in stem.chars() {
        let out = if ch.is_ascii_alphanumeric() || ch == '_' {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' || ch.is_whitespace() {
            Some('-')
        } else {
            None
        };
        if let Some(out) = out {
            if out == '-' {
                if !previous_dash && !normalized.is_empty() {
                    normalized.push(out);
                }
                previous_dash = true;
            } else {
                normalized.push(out);
                previous_dash = false;
            }
        }
    }
    while normalized.ends_with('-') {
        normalized.pop();
    }
    (!normalized.is_empty()).then(|| format!("{normalized}.lisp"))
}

pub(super) fn open_script_source_buffer(editor: &mut Editor, path: &Path) -> Result<String, String> {
    if let Some(existing) = editor.buffers.iter().find(|buffer| {
        buffer
            .path
            .as_ref()
            .is_some_and(|existing| existing == path)
    }) {
        return Ok(existing.name.clone());
    }

    let internal_name = script_source_buffer_name(path);
    if let Some(existing) = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == internal_name)
    {
        if existing
            .path
            .as_ref()
            .is_some_and(|existing| existing == path)
        {
            return Ok(existing.name.clone());
        }
        editor.remove_buffer_by_name(&internal_name);
    }

    editor
        .create_file_buffer(path, BufferMode::ESeqLisp)
        .map_err(|error| {
            format!(
                "Failed to open script source '{}': {error:?}",
                path.display()
            )
        })?;
    let idx = editor
        .buffers
        .iter()
        .rposition(|buffer| {
            buffer
                .path
                .as_ref()
                .is_some_and(|existing| existing == path)
        })
        .ok_or_else(|| {
            format!(
                "Opened script source '{}' but no buffer was created",
                path.display()
            )
        })?;
    editor.buffers[idx].name = internal_name.clone();
    Ok(internal_name)
}

pub(super) fn register_script_source_tab(
    editor: &mut Editor,
    path: &Path,
    label: &str,
    source_path_for_project: &str,
) -> Result<String, String> {
    let buffer_name = open_script_source_buffer(editor, path)?;
    let label = escape_lisp_string(label);
    let buffer = escape_lisp_string(&buffer_name);
    let source_path = escape_lisp_string(source_path_for_project);
    let form = format!(
        "(seq-register-script-step-sequencer-tab \"{label}\" \"{buffer}\" \"\" \"{source_path}\")"
    );
    editor
        .runtime_mut()
        .eval_str(&form)
        .map_err(|error| format!("Failed to register script source tab: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    Ok(buffer_name)
}

pub(super) fn preserve_sample_browser_context_for_loaded_sample(editor: &mut Editor, path: &str) {
    let path = escape_lisp_string(path);
    if let Err(error) = editor
        .runtime_mut()
        .eval_str(&format!("(set! sbrowser-auditioned-sample \"{path}\")"))
    {
        eprintln!("sample browser: failed to mark browser-initiated sample load: {error:?}");
    }
}

pub(super) fn refresh_sample_browser_buffer(editor: &mut Editor) -> Result<(), String> {
    editor
        .runtime_mut()
        .eval_str("(sbrowser-refresh-buffer)")
        .map_err(|error| format!("{error:?}"))?;
    editor.refresh_runtime_side_effects();
    editor.refresh_visible_layouts_for_buffer_named("*samples*");
    editor.mark_needs_redraw();
    Ok(())
}

impl ActiveDeleteTarget {
    pub(super) fn buffer_name(&self) -> &'static str {
        match self {
            ActiveDeleteTarget::MixerTrack { .. }
            | ActiveDeleteTarget::TrackPattern { .. }
            | ActiveDeleteTarget::ModRoute { .. } => "*mixer*",
            ActiveDeleteTarget::FxEffect { .. }
            | ActiveDeleteTarget::RackEffect { .. }
            | ActiveDeleteTarget::RackSlot { .. } => "*fx*",
        }
    }
}

pub(super) fn should_clear_active_delete_target_for_buffer(
    target: Option<&ActiveDeleteTarget>,
    active_buffer_name: &str,
) -> bool {
    target.is_some_and(|target| target.buffer_name() != active_buffer_name)
}

pub(super) fn pull_shared_bus_state(
    app: &mut app::App,
    bus_state: &Arc<Mutex<Vec<app::BusChannelState>>>,
) -> bool {
    let latest = bus_state.lock().unwrap();
    if app.buses.len() != latest.len() {
        app.buses = latest.clone();
        return true;
    }

    let changed = app
        .buses
        .iter()
        .zip(latest.iter())
        .any(|(bus, latest_bus)| {
            bus.volume != latest_bus.volume
                || bus.mute != latest_bus.mute
                || bus.solo != latest_bus.solo
        });
    if !changed {
        return false;
    }

    for (bus, latest_bus) in app.buses.iter_mut().zip(latest.iter()) {
        bus.volume = latest_bus.volume;
        bus.mute = latest_bus.mute;
        bus.solo = latest_bus.solo;
    }
    true
}

pub(super) fn editor_has_visible_buffer(editor: &Editor, name: &str) -> bool {
    editor.tile_root.leaf_ids().into_iter().any(|tile_id| {
        editor
            .tile_root
            .find_leaf(tile_id)
            .and_then(|leaf| editor.buffers.get(leaf.buffer_idx))
            .is_some_and(|buffer| buffer.name == name && buffer.view_mode != ViewMode::TextOnly)
    })
}

pub(super) fn track_meter_bindings_visible(mixer_visible: bool, sequencer_visible: bool) -> bool {
    mixer_visible || sequencer_visible
}

/// `*patch-mixer*` (the reduced strip in the patch editor) renders the same
/// meter/strip bindings as `*mixer*`, so anything gated on mixer visibility
/// must treat either buffer as "the mixer is on screen".
pub(super) fn editor_has_visible_mixer_buffer(editor: &Editor) -> bool {
    editor_has_visible_buffer(editor, "*mixer*")
        || editor_has_visible_buffer(editor, "*patch-mixer*")
}

pub(super) fn refresh_visible_mixer_layouts(editor: &mut Editor) {
    editor.refresh_visible_layouts_for_buffer_named("*mixer*");
    editor.refresh_visible_layouts_for_buffer_named("*patch-mixer*");
}

pub(super) fn reconciled_track_index(
    stored_track: usize,
    cursor_track: usize,
    track_count: usize,
) -> Option<usize> {
    if track_count == 0 {
        return None;
    }
    if stored_track < track_count {
        Some(stored_track)
    } else if cursor_track < track_count {
        Some(cursor_track)
    } else {
        Some(track_count - 1)
    }
}

pub(super) fn current_track_for_app(app: &mut app::App, current_track: &Arc<AtomicUsize>) -> Option<usize> {
    let track = reconciled_track_index(
        current_track.load(Ordering::Relaxed),
        app.ui.cursor_track,
        app.tracks.len(),
    )?;
    if current_track.load(Ordering::Relaxed) != track {
        current_track.store(track, Ordering::Relaxed);
    }
    app.ui.cursor_track = track;
    Some(track)
}

pub(super) fn reveal_sequencer_current_track(editor: &mut Editor, app: &app::App, track: usize) {
    let Some(track_id) = app.graph.track_node_ids.get(track).map(|ids| ids.pan_id) else {
        return;
    };
    let key = format!("sequencer-track-{track_id}");
    if !editor.visible_buffer_layout_contains_stable_key("*sequencer*", &key) {
        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
    }
    editor.ensure_widget_stable_key_visible_in_buffer_named("*sequencer*", &key, 1.0);
}

pub(super) fn key_should_reveal_sequencer_track(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(
        (key.code, key.modifiers),
        (KeyCode::Up, KeyModifiers::NONE)
            | (KeyCode::Down, KeyModifiers::NONE)
    )
}

pub(super) fn track_button_state_snapshot(state: &Arc<SequencerState>) -> Vec<(bool, bool)> {
    (0..state.active_track_count())
        .map(|track| {
            let params = &state.pattern.track_params[track];
            (params.is_muted(), params.is_solo())
        })
        .collect()
}

pub(super) fn param_unit(param: &sequencer::effects::ParamDescriptor) -> Option<String> {
    match &param.kind {
        ParamKind::Continuous { unit } => unit.clone(),
        _ => None,
    }
}

pub(super) fn param_enum_labels(param: &sequencer::effects::ParamDescriptor) -> Vec<String> {
    match &param.kind {
        ParamKind::Enum { labels } => labels.clone(),
        _ => Vec::new(),
    }
}

pub(super) fn param_scaling(param: &sequencer::effects::ParamDescriptor) -> String {
    match param.scaling {
        ParamScaling::Linear => "linear".to_string(),
        ParamScaling::Exponential => "exponential".to_string(),
    }
}

pub(super) fn metal_agent_session_context(
    app: &app::App,
    current_track: &Arc<AtomicUsize>,
    active_delete_target: &Arc<Mutex<Option<ActiveDeleteTarget>>>,
) -> AgentSessionContext {
    let track = current_track_for_snapshot(app, current_track);
    let current_track_name = track.and_then(|track| app.tracks.get(track).cloned());
    let current_instrument_name = track.and_then(|track| {
        if app.graph.track_instrument_types.get(track)
            != Some(&sequencer::sequencer::InstrumentType::Custom)
        {
            return None;
        }
        current_custom_instrument_name(app, track)
    });
    let current_instrument_source = current_instrument_name
        .as_deref()
        .and_then(|name| sequencer::lisp_host::load_instrument_source(name).ok());
    let current_instrument_preset_schema =
        metal_agent_instrument_preset_schema(app, track, current_instrument_name.as_deref());

    let current_effect_slot = active_delete_target
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|target| match target {
            ActiveDeleteTarget::FxEffect {
                chain: FxDeleteChain::Audio,
                slot,
                ..
            } if *slot >= sequencer::effects::BUILTIN_SLOT_COUNT => Some(*slot),
            _ => None,
        });
    let current_effect_name = track.and_then(|track| {
        current_effect_slot.and_then(|slot| {
            app.graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(slot))
                .map(|desc| desc.name.clone())
        })
    });
    let current_effect_source = current_effect_name
        .as_deref()
        .and_then(|name| sequencer::lisp_host::load_effect_source(name).ok());
    let current_effect_ui_source = current_effect_name
        .as_deref()
        .and_then(|name| sequencer::lisp_host::load_effect_ui_source(name).ok());

    AgentSessionContext {
        has_tracks: !app.tracks.is_empty(),
        current_track_name,
        current_track_index: track,
        can_apply_effect_to_current_track: app.next_free_custom_slot().is_some(),
        current_effect_name,
        current_effect_source: current_effect_source.clone(),
        current_effect_ui_source,
        current_effect_slot,
        can_update_current_effect: current_effect_source.is_some(),
        current_instrument_name,
        current_instrument_source: current_instrument_source.clone(),
        can_update_current_instrument: current_instrument_source.is_some(),
        current_instrument_preset_schema,
    }
}

pub(super) fn current_track_for_snapshot(app: &app::App, current_track: &Arc<AtomicUsize>) -> Option<usize> {
    if app.tracks.is_empty() {
        return None;
    }
    let stored = current_track.load(Ordering::Relaxed);
    if stored < app.tracks.len() {
        Some(stored)
    } else {
        Some(app.tracks.len() - 1)
    }
}

pub(super) fn metal_agent_instrument_preset_schema(
    app: &app::App,
    track: Option<usize>,
    instrument_name: Option<&str>,
) -> Option<AgentInstrumentPresetSchema> {
    let track = track?;
    let instrument_name = instrument_name?;
    let desc = app.graph.instrument_descriptors.get(track)?;
    let slot = app.state.pattern.instrument_slots.get(track)?;
    let existing_presets = sequencer::lisp_host::load_instrument_presets(instrument_name)
        .map(|presets| presets.into_iter().map(|preset| preset.name).collect())
        .unwrap_or_default();

    let mut params = Vec::new();
    for (idx, param) in desc.params.iter().enumerate() {
        let group = if param.node_param_idx >= sequencer::instruments::voice_modulator::MOD_PARAM_BASE {
            "source"
        } else if param.name.starts_with("mod ") {
            "mod"
        } else {
            "synth"
        };
        params.push(AgentInstrumentParamSchema {
            name: param.name.clone(),
            group: group.to_string(),
            min: param.min,
            max: param.max,
            default: param.default,
            current_value: Some(slot.defaults.get(idx)),
            unit: param_unit(param),
            enum_labels: param_enum_labels(param),
            scaling: param_scaling(param),
        });
    }

    Some(AgentInstrumentPresetSchema {
        instrument_name: instrument_name.to_string(),
        source_file: sequencer::lisp_host::instrument_source_path(instrument_name)
            .ok()
            .map(|path| path.display().to_string()),
        base_note_offset: f32::from_bits(
            app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        ),
        existing_presets,
        params,
    })
}

#[derive(Default)]
pub(super) struct StubAnimationRenderCache {
    pub(super) frame: Option<eseqlisp::backend::TiledRenderFrame>,
    pub(super) size: Option<(usize, usize)>,
}

impl StubAnimationRenderCache {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn reset(&mut self) {
        self.frame = None;
        self.size = None;
    }

    pub(super) fn update_size(&mut self, size: (usize, usize)) {
        if self.size != Some(size) {
            self.reset();
        }
    }

    pub(super) fn store(&mut self, size: (usize, usize), frame: eseqlisp::backend::TiledRenderFrame) {
        self.size = Some(size);
        self.frame = Some(frame);
    }

    pub(super) fn is_active(&self, size: (usize, usize), stub_visible: bool) -> bool {
        stub_visible && self.size == Some(size) && self.frame.is_some()
    }

    pub(super) fn frame(&self) -> Option<&eseqlisp::backend::TiledRenderFrame> {
        self.frame.as_ref()
    }
}
