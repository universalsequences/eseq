use crate::*;

const PACKAGES_BUFFER_NAME: &str = "*packages*";
const LISTING_START_LINE: usize = 5;

pub(super) const COMMANDS: &[&str] = &[
    "open-packages-view",
    "packages-view-key",
    "menu-import-package",
    "package-import-stage",
    "package-import-commit",
    "package-import-cancel",
    "menu-export-package",
    "package-export-commit",
];

// ── Export Package… ──
//
// The export modal (`eseq.file-dialogs/package-export-body`) lists every
// user-tier instrument and effect from `seq-package-export-candidates`; the
// picked set lives here so Lisp only toggles and re-reads it. Commit asks
// for a destination with the native save panel, writes the pack with
// `sequencer::package_export::export_package`, and reports.

thread_local! {
    static EXPORT_SELECTION: std::cell::RefCell<
        std::collections::BTreeSet<(sequencer::package_export::ExportKind, String)>,
    > = const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

pub(crate) fn register_package_export_natives(runtime: &mut Runtime) {
    // Every exportable item as (dict :kind "instrument"|"effect" :name
    // <logical> :selected? bool), instruments first.
    // An optional kind argument ("instrument" | "effect" | "presets") narrows
    // the list so a column never renders placeholder rows for other kinds.
    runtime.register_native("seq-package-export-candidates", |args, _ctx| {
        let only = match args.first() {
            Some(Value::String(kind)) => Some(
                sequencer::package_export::ExportKind::parse(kind).ok_or("unknown export kind")?,
            ),
            _ => None,
        };
        let candidates =
            sequencer::package_export::export_candidates(sequencer::app_paths::app_paths())
                .into_iter()
                .filter(|candidate| only.is_none_or(|kind| candidate.kind == kind))
                .collect::<Vec<_>>();
        let items = EXPORT_SELECTION.with(|selection| {
            let selection = selection.borrow();
            candidates
                .into_iter()
                .map(|candidate| {
                    let selected =
                        selection.contains(&(candidate.kind, candidate.logical.clone()));
                    Rc::new(RefCell::new(crate::values::map_value([
                        ("kind", Value::String(candidate.kind.name().into())),
                        ("name", Value::String(candidate.logical)),
                        ("selected?", Value::Bool(selected)),
                    ])))
                })
                .collect::<Vec<_>>()
        });
        Ok(Value::List(items))
    });
    // (seq-package-export-toggle kind name) flips one item; returns the new
    // selected count so the caller can bump its generation.
    runtime.register_native("seq-package-export-toggle", |args, _ctx| {
        let (Some(Value::String(kind)), Some(Value::String(name))) = (args.first(), args.get(1))
        else {
            return Err("seq-package-export-toggle expects kind and name".into());
        };
        let kind = sequencer::package_export::ExportKind::parse(kind).ok_or("unknown export kind")?;
        let count = EXPORT_SELECTION.with(|selection| {
            let mut selection = selection.borrow_mut();
            let key = (kind, name.clone());
            if !selection.remove(&key) {
                selection.insert(key);
            }
            selection.len()
        });
        Ok(Value::Number(count as f64))
    });
    runtime.register_native("seq-package-export-selected-count", |_args, _ctx| {
        Ok(Value::Number(EXPORT_SELECTION.with(|selection| selection.borrow().len()) as f64))
    });
    runtime.register_native("seq-package-export-clear", |_args, _ctx| {
        EXPORT_SELECTION.with(|selection| selection.borrow_mut().clear());
        Ok(Value::Nil)
    });
}

fn commit_package_export(payload: &Value) -> Result<String, String> {
    let Value::Map(map) = payload else {
        return Err("Expected :identity and :version".into());
    };
    let identity = map_string(map, "identity").unwrap_or_default();
    let identity = identity.trim().to_string();
    let version = map_string(map, "version").unwrap_or_default();
    let version = version.trim().to_string();
    eseqlisp::package::validate_package_name(&identity)
        .map_err(|error| format!("package name: {error}"))?;
    if version.is_empty() {
        return Err("Enter a version".into());
    }
    let items = EXPORT_SELECTION.with(|selection| selection.borrow().iter().cloned().collect::<Vec<_>>());
    if items.is_empty() {
        return Err("Pick at least one instrument or effect".into());
    }
    let suggested = sequencer::package_export::archive_file_name(&identity, &version);
    let Some(archive_path) = crate::application_menu::choose_package_export_path(&suggested)? else {
        return Err("Export canceled".into());
    };
    let out_dir = archive_path
        .parent()
        .ok_or("Choose a folder for the archive")?
        .to_path_buf();
    // The writer names the archive itself; the panel only picks the folder
    // (and lets the user rename the file, which we honor by renaming after).
    let request = sequencer::package_export::ExportRequest { identity, version, items };
    let report = sequencer::package_export::export_package(
        sequencer::app_paths::app_paths(),
        &request,
        &out_dir,
        true,
    )?;
    let mut archive = report.archive.clone().ok_or("export produced no archive")?;
    if archive != archive_path {
        if archive_path.exists() {
            let _ = std::fs::remove_dir_all(&report.package_dir);
            let _ = std::fs::remove_file(&archive);
            return Err(format!("{} already exists", archive_path.display()));
        }
        std::fs::rename(&archive, &archive_path).map_err(|error| error.to_string())?;
        archive = archive_path;
    }
    // The archive is the deliverable; the unpacked folder beside it would
    // only confuse a later drag into the packages directory.
    let _ = std::fs::remove_dir_all(&report.package_dir);
    EXPORT_SELECTION.with(|selection| selection.borrow_mut().clear());
    let mut message = format!(
        "Exported {} ({} instrument(s), {} effect(s), {} preset bank(s))",
        archive.display(),
        report.instruments,
        report.effects,
        report.presets
    );
    for warning in &report.warnings {
        eprintln!("metal_seq: export warning: {warning}");
    }
    if let Some(first) = report.warnings.first() {
        message.push_str(&format!(" — warning: {first}"));
    }
    Ok(message)
}

// ── Import Package… ──
//
// File > Import Package… picks a folder or archive, stages and validates it
// (`sequencer::package_install::stage_package_from_path`), and opens the
// confirmation modal in `eseq.file-dialogs`. The staged package waits in
// this slot until the user installs or cancels; a leaked staging directory
// is hidden from the package scan anyway.

thread_local! {
    static STAGED_PACKAGE: std::cell::RefCell<Option<sequencer::package_install::StagedPackage>> =
        const { std::cell::RefCell::new(None) };
}

fn install_staged(staged: sequencer::package_install::StagedPackage) {
    STAGED_PACKAGE.with(|slot| {
        if let Some(previous) = slot.borrow_mut().replace(staged) {
            sequencer::package_install::discard_staged_package(previous);
        }
    });
}

fn take_staged() -> Option<sequencer::package_install::StagedPackage> {
    STAGED_PACKAGE.with(|slot| slot.borrow_mut().take())
}

#[cfg(test)]
pub(crate) fn install_staged_for_tests(staged: sequencer::package_install::StagedPackage) {
    install_staged(staged);
}

#[cfg(test)]
pub(crate) fn take_staged_for_tests() -> Option<sequencer::package_install::StagedPackage> {
    take_staged()
}

/// The confirmation modal's view of the staged package: a dict of counts
/// (see `eseq.file-dialogs/package-import-body`), or `nil` when nothing is
/// staged.
pub(crate) fn staged_package_summary_value() -> Value {
    STAGED_PACKAGE.with(|slot| {
        let slot = slot.borrow();
        let Some(staged) = slot.as_ref() else {
            return Value::Nil;
        };
        let summary = &staged.summary;
        let number = |value: usize| Value::Number(value as f64);
        crate::values::map_value([
            ("identity", Value::String(summary.identity.clone())),
            ("version", Value::String(summary.version.clone())),
            ("path", Value::String(staged.destination().display().to_string())),
            ("modules", number(summary.modules)),
            ("instruments", number(summary.instruments)),
            ("effects", number(summary.effects)),
            ("midi-fx", number(summary.midi_fx)),
            ("samples", number(summary.samples)),
            ("themes", number(summary.themes)),
            ("presets", number(summary.presets)),
            ("installed?", Value::Bool(staged.replaces_installed())),
        ])
    })
}

pub(crate) fn register_package_import_natives(runtime: &mut Runtime) {
    runtime.register_native("seq-package-import-summary", |_args, _ctx| {
        Ok(staged_package_summary_value())
    });
    // Script entry point (capture fixtures, automation): stage a package
    // folder or archive exactly as the menu command would, without the file
    // dialog. Returns the identity; the caller opens the modal.
    runtime.register_native("seq-package-import-stage", |args, _ctx| {
        let Some(Value::String(path)) = args.first() else {
            return Err("seq-package-import-stage expects a path string".into());
        };
        let packages_dir = sequencer::app_paths::app_paths().packages_dir();
        let staged =
            sequencer::package_install::stage_package_from_path(Path::new(path), &packages_dir)?;
        let identity = staged.identity().to_string();
        install_staged(staged);
        Ok(Value::String(identity))
    });
}

/// Stage `source` and open the confirmation modal. Shared by the menu
/// command (after the file dialog) and the scripted `package-import-stage`
/// entry point that capture fixtures and tests use.
fn stage_and_open(source: &Path, editor: &mut Editor) -> Result<(), String> {
    let packages_dir = sequencer::app_paths::app_paths().packages_dir();
    let staged = sequencer::package_install::stage_package_from_path(source, &packages_dir)?;
    let description = format!(
        "{} {}: {}",
        staged.summary.identity,
        staged.summary.version,
        staged.summary.describe_contents()
    );
    install_staged(staged);
    super::file_menu::activate_dialog_tile(editor);
    editor
        .runtime_mut()
        .eval_str("(eseq.file-dialogs/open-package-import)")
        .map_err(|error| format!("{error:?}"))?;
    editor.show_transient_message(format!("Staged {description}"));
    Ok(())
}

fn close_package_import_modal(editor: &mut Editor) {
    let _ = editor
        .runtime_mut()
        .eval_str("(eseq.file-dialogs/close-package-import)");
}

/// Publish the staged package and make it live without a relaunch: the
/// package catalog cache is dropped, the UI runtime's module load path is
/// rebuilt, the scratch source is republished so the scheduler and MIDI-fx
/// runtimes rebuild theirs (they construct a fresh runtime from
/// `module_load_roots` on every scratch version), package samples are
/// reconciled into the sample DB, and the browsers re-list.
fn commit_staged_package(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<String, String> {
    let staged = take_staged().ok_or("Nothing is staged for import")?;
    let replace = staged.replaces_installed();
    let summary = staged.summary.clone();
    let result = sequencer::package_install::publish_staged_package(staged, replace)?;
    register_installed_packages_live(app, editor, ctx);
    let mut message = format!(
        "{} {} {}: {}",
        if replace { "Replaced" } else { "Installed" },
        summary.identity,
        summary.version,
        summary.describe_contents()
    );
    if let Some(error) = reconcile_package_samples_after_change() {
        message.push_str(&format!(" (samples: {error})"));
    }
    let _ = result;
    Ok(message)
}

/// Re-point every runtime at the current package set. Safe to call after any
/// install, replace, or removal.
pub(crate) fn register_installed_packages_live(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    sequencer::app_paths::invalidate_package_catalog_cache();
    let app_paths = sequencer::app_paths::app_paths();
    let (roots, errors) = app_paths.module_load_roots();
    for error in errors {
        eprintln!("metal_seq: {error}");
    }
    editor.runtime_mut().set_scoped_module_load_path(roots);
    // Same source, new version: the scheduler and MIDI-fx runtimes rebuild
    // from `module_load_roots` on the next tick.
    let scratch = app.state.scratch_source();
    app.state.set_scratch_source(scratch);
    ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    let _ = editor.runtime_mut().eval_str("(eseq.browser/refresh-buffer)");
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

fn reconcile_package_samples_after_change() -> Option<String> {
    match sequencer::package_samples::reconcile_app_package_samples(
        sequencer::app_paths::app_paths(),
    ) {
        Ok(report) => {
            for error in &report.errors {
                eprintln!("metal_seq: {error}");
            }
            report.errors.first().cloned()
        }
        Err(error) => Some(error),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageEntry {
    path: PathBuf,
    name: String,
    module: Option<String>,
    directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CreateTarget {
    Module { name: String, path: PathBuf },
    Directory(PathBuf),
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    match name {
        "open-packages-view" => open_packages_view(editor, ctx),
        "packages-view-key" => handle_packages_key(&payload, app, editor, ctx),
        "menu-import-package" => {
            let result = (|| -> Result<(), String> {
                let Some(source) = crate::application_menu::choose_package_path()? else {
                    return Ok(());
                };
                stage_and_open(&source, editor)
            })();
            if let Err(error) = result {
                editor.show_transient_message(format!("Import package: {error}"));
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "package-import-stage" => {
            let result = (|| -> Result<(), String> {
                let Value::Map(map) = &payload else {
                    return Err("Expected a :path".into());
                };
                let path = map_string(map, "path").ok_or("Expected a :path")?;
                stage_and_open(Path::new(&path), editor)
            })();
            if let Err(error) = result {
                editor.show_transient_message(format!("Import package: {error}"));
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "package-import-commit" => {
            match commit_staged_package(app, editor, ctx) {
                Ok(message) => {
                    close_package_import_modal(editor);
                    editor.show_transient_message(message);
                }
                Err(error) => {
                    close_package_import_modal(editor);
                    editor.show_transient_message(format!("Import package failed: {error}"));
                }
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "menu-export-package" => {
            super::file_menu::activate_dialog_tile(editor);
            if let Err(error) = editor
                .runtime_mut()
                .eval_str("(eseq.file-dialogs/open-package-export)")
            {
                editor.show_transient_message(format!("Export package: {error:?}"));
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "package-export-commit" => {
            match commit_package_export(&payload) {
                Ok(message) => {
                    let _ = editor
                        .runtime_mut()
                        .eval_str("(eseq.file-dialogs/close-package-export)");
                    editor.show_transient_message(message);
                }
                Err(error) => editor.show_transient_message(format!("Export package: {error}")),
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "package-import-cancel" => {
            if let Some(staged) = take_staged() {
                sequencer::package_install::discard_staged_package(staged);
            }
            close_package_import_modal(editor);
            editor.show_transient_message("Package import canceled");
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        _ => {}
    }
}

fn open_packages_view(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    if ctx.sessions.package_view_session.is_some() {
        let _ = editor.switch_active_tile_to_buffer_named(PACKAGES_BUFFER_NAME);
        return;
    }

    let previous_buffer = visible_main_panel_buffer(editor)
        .filter(|name| editor.switch_active_tile_to_buffer_named(name))
        .unwrap_or_else(|| "*sequencer*".to_string());
    if !editor.switch_active_tile_to_buffer_named(&previous_buffer) {
        editor.handle_host_event(HostEvent::Status(
            "Packages view could not find the main sequencer panel".to_string(),
        ));
        return;
    }

    let root = sequencer::app_paths::app_paths().local_modules_dir();
    if let Err(error) = std::fs::create_dir_all(&root) {
        editor.handle_host_event(HostEvent::Status(format!(
            "Could not create packages directory '{}': {error}",
            root.display()
        )));
        return;
    }

    if editor
        .buffers
        .iter()
        .all(|buffer| buffer.name != PACKAGES_BUFFER_NAME)
    {
        editor.create_scratch_buffer(PACKAGES_BUFFER_NAME, "", BufferMode::ESeqLisp);
    }
    if !editor.swap_buffer_in_tile_showing(&previous_buffer, PACKAGES_BUFFER_NAME) {
        editor.handle_host_event(HostEvent::Status(
            "Packages view could not replace the main sequencer panel".to_string(),
        ));
        return;
    }
    let _ = editor.switch_active_tile_to_buffer_named(PACKAGES_BUFFER_NAME);

    ctx.sessions.package_view_session = Some(PackageViewSession {
        root,
        current_dir: PathBuf::new(),
        query: String::new(),
        selected: 0,
        previous_buffer,
    });
    if let Err(error) = set_packages_buffer_mode(editor) {
        close_packages_view(editor, ctx);
        editor.handle_host_event(HostEvent::Status(error));
        return;
    }
    refresh_packages_view(editor, ctx);
}

fn visible_main_panel_buffer(editor: &mut Editor) -> Option<String> {
    match editor
        .runtime_mut()
        .eval_str("(eseq.seq-step-tabs/seq-visible-main-panel-buffer)")
    {
        Ok(Some(Value::String(name))) => Some(name),
        _ => Some("*sequencer*".to_string()),
    }
}

fn set_packages_buffer_mode(editor: &mut Editor) -> Result<(), String> {
    editor
        .runtime_mut()
        .eval_str("(set-buffer-mode-for \"*packages*\" \"eseq.packages/packages-mode\")")
        .map_err(|error| format!("Could not activate Packages mode: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    if !editor.set_buffer_view_mode_by_name(PACKAGES_BUFFER_NAME, ViewMode::TextOnly) {
        return Err("Could not activate the Packages text buffer".to_string());
    }
    Ok(())
}

fn handle_packages_key(
    payload: &Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Some(key) = extract_string_from_payload(payload, "key") else {
        return;
    };
    let Some(session) = ctx.sessions.package_view_session.as_mut() else {
        return;
    };

    match key.as_str() {
        "ESC" => {
            close_packages_view(editor, ctx);
            return;
        }
        "q" if session.query.is_empty() => {
            close_packages_view(editor, ctx);
            return;
        }
        "C-g" => {}
        "UP" => session.selected = session.selected.saturating_sub(1),
        "DOWN" => session.selected = session.selected.saturating_add(1),
        "BS" => {
            session.query.pop();
            session.selected = 0;
        }
        "-" if session.query.is_empty() => {
            session.current_dir.pop();
            session.selected = 0;
        }
        "RET" => {
            activate_packages_selection(editor, ctx);
            return;
        }
        "C-a" => {
            attach_selected_package(editor, app, ctx, AttachmentDestination::Scratch);
            return;
        }
        "C-i" => {
            attach_selected_package(editor, app, ctx, AttachmentDestination::UserInit);
            return;
        }
        "C-j" => {
            open_user_init(editor, ctx);
            return;
        }
        _ => {
            if let Some(text) = extract_string_from_payload(payload, "text") {
                if text.chars().count() == 1 && !text.chars().any(char::is_control) {
                    session.query.push_str(&text);
                    session.selected = 0;
                }
            }
        }
    }
    refresh_packages_view(editor, ctx);
}

fn activate_packages_selection(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let Some(session) = ctx.sessions.package_view_session.as_ref() else {
        return;
    };
    let entries = filtered_entries(session);
    let exact_entry = exact_entry_index(&entries, &session.query);
    if !session.query.trim().is_empty() && exact_entry.is_none() {
        match create_target(&session.root, &session.query) {
            Ok(CreateTarget::Module { name, path }) if !path.exists() => {
                match create_module_file(&path, &name) {
                    Ok(()) => open_package_file(editor, ctx, &path),
                    Err(error) => editor.handle_host_event(HostEvent::Status(error)),
                }
                return;
            }
            Ok(CreateTarget::Directory(path)) if !path.exists() => {
                match std::fs::create_dir_all(&path) {
                    Ok(()) => {
                        if let Some(session) = ctx.sessions.package_view_session.as_mut() {
                            session.query.clear();
                            session.selected = 0;
                        }
                        refresh_packages_view(editor, ctx);
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Could not create folder '{}': {error}",
                        path.display()
                    ))),
                }
                return;
            }
            _ => {}
        }
    }

    let selected = exact_entry.unwrap_or(session.selected);
    let Some(entry) = entries.get(selected.min(entries.len().saturating_sub(1))) else {
        editor.handle_host_event(HostEvent::Status(
            "Type a module name or select a module".to_string(),
        ));
        return;
    };
    if entry.directory {
        let relative = entry
            .path
            .strip_prefix(&session.root)
            .unwrap_or(&entry.path)
            .to_path_buf();
        if let Some(session) = ctx.sessions.package_view_session.as_mut() {
            session.current_dir = relative;
            session.query.clear();
            session.selected = 0;
        }
        refresh_packages_view(editor, ctx);
    } else {
        open_package_file(editor, ctx, &entry.path);
    }
}

fn open_package_file(editor: &mut Editor, ctx: &mut LoopCtx<'_>, path: &Path) {
    match editor.open_or_create_file_buffer(path) {
        Ok(_) => finish_packages_session(editor, ctx),
        Err(error) => editor.handle_host_event(HostEvent::Status(format!(
            "Could not open '{}': {error:?}",
            path.display()
        ))),
    }
}

fn close_packages_view(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let Some(session) = ctx.sessions.package_view_session.take() else {
        return;
    };
    let _ = editor.swap_buffer_in_tile_showing(PACKAGES_BUFFER_NAME, &session.previous_buffer);
    editor.remove_buffer_by_name(PACKAGES_BUFFER_NAME);
    let _ = editor.switch_active_tile_to_buffer_named(&session.previous_buffer);
    reinstall_step_tabs(editor);
}

fn finish_packages_session(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    ctx.sessions.package_view_session = None;
    editor.remove_buffer_by_name(PACKAGES_BUFFER_NAME);
    reinstall_step_tabs(editor);
}

/// Reinstall the step-tab bar once the sequencer is back on screen.
///
/// `set-window-tabs-for` no-ops when no tile is showing the target buffer, and
/// the Packages view takes over the sequencer's tile. A module attached from
/// the view registers its step tab while *sequencer* is off screen, so the
/// refresh its registration triggers cannot install anything — without this
/// the tab is registered but never appears.
fn reinstall_step_tabs(editor: &mut Editor) {
    let _ = editor
        .runtime_mut()
        .eval_str("(eseq.seq-step-tabs/seq-refresh-step-tabs-if-present)");
    editor.refresh_runtime_side_effects();
}

fn refresh_packages_view(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let Some(session) = ctx.sessions.package_view_session.as_mut() else {
        return;
    };
    let entries = filtered_entries(session);
    session.selected = session.selected.min(entries.len().saturating_sub(1));

    let scratch =
        buffer_source(editor, Path::new(""), PROJECT_SCRATCH_BUFFER_NAME).unwrap_or_default();
    let init_path = user_init_path();
    let init = buffer_source(editor, &init_path, "")
        .or_else(|| std::fs::read_to_string(&init_path).ok())
        .unwrap_or_default();
    let scratch_imports = import_modules(&scratch);
    let init_imports = import_modules(&init);

    let mut lines = vec![
        format!(
            "Packages  {}",
            session.root.join(&session.current_dir).display()
        ),
        format!("Name/filter: {}", session.query),
        preview_line(&session.root, &session.query),
        "RET open/create  C-a project ✓  C-i every session ★  C-j init.lisp  - parent  Esc quit"
            .to_string(),
        String::new(),
    ];
    for entry in &entries {
        if entry.directory {
            lines.push(format!("    {}/", entry.name));
        } else {
            let module = entry.module.as_deref().unwrap_or("(no module header)");
            let scratch_mark = if scratch_imports.contains(module) {
                '✓'
            } else {
                ' '
            };
            let init_mark = if init_imports.contains(module) {
                '★'
            } else {
                ' '
            };
            lines.push(format!(
                "[{scratch_mark}{init_mark}] {:<32} {module}",
                entry.name
            ));
        }
    }
    if entries.is_empty() {
        let message = if session.query.is_empty() {
            "    No local modules yet — type a name and press RET"
        } else {
            "    No matching module — press RET to create it"
        };
        lines.push(message.to_string());
    }

    if let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.name == PACKAGES_BUFFER_NAME)
    {
        buffer.read_only = false;
        buffer.set_text(&lines.join("\n"));
        buffer.read_only = true;
        buffer.dirty = false;
        buffer.cursor = (LISTING_START_LINE + session.selected, 0);
    }
    editor.mark_needs_redraw();
}

fn filtered_entries(session: &PackageViewSession) -> Vec<PackageEntry> {
    let directory = session.root.join(&session.current_dir);
    let mut entries = scan_directory(&directory).unwrap_or_default();
    let query = session
        .query
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase();
    if !query.is_empty() {
        entries.retain(|entry| {
            entry.name.to_ascii_lowercase().contains(&query)
                || entry
                    .module
                    .as_ref()
                    .is_some_and(|module| module.to_ascii_lowercase().contains(&query))
        });
    }
    entries
}

fn exact_entry_index(entries: &[PackageEntry], query: &str) -> Option<usize> {
    let query = query.trim().trim_end_matches('/');
    let file_query = query.strip_suffix(".lisp").unwrap_or(query);
    entries.iter().position(|entry| {
        entry.name == query
            || entry.name.strip_suffix(".lisp") == Some(file_query)
            || entry.module.as_deref() == Some(file_query)
    })
}

fn scan_directory(directory: &Path) -> Result<Vec<PackageEntry>, String> {
    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(directory)
        .map_err(|error| format!("Could not read '{}': {error}", directory.display()))?;
    for item in read_dir {
        let item = item.map_err(|error| format!("Could not read local module entry: {error}"))?;
        let path = item.path();
        let file_type = item
            .file_type()
            .map_err(|error| format!("Could not inspect '{}': {error}", path.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            entries.push(PackageEntry {
                path,
                name,
                module: None,
                directory: true,
            });
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "lisp") {
            let module = std::fs::read_to_string(&path)
                .ok()
                .and_then(|source| declared_module(&source));
            entries.push(PackageEntry {
                path,
                name,
                module,
                directory: false,
            });
        }
    }
    entries.sort_by(|left, right| {
        right.directory.cmp(&left.directory).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    Ok(entries)
}

fn declared_module(source: &str) -> Option<String> {
    fn from_expression(expression: Expression) -> Option<String> {
        match expression {
            Expression::List(items) => match items.as_slice() {
                [Expression::Symbol(head), Expression::Symbol(name), ..] if head == "module" => {
                    Some(name.clone())
                }
                _ => None,
            },
            _ => None,
        }
    }

    let whole_source = Parser::new(source.to_string())
        .parse()
        .ok()
        .and_then(|tokens| ASTParser::new(tokens).parse().ok())
        .and_then(|expressions| expressions.into_iter().find_map(from_expression));
    whole_source.or_else(|| {
        source.lines().find_map(|line| {
            Parser::new(line.to_string())
                .parse()
                .ok()
                .and_then(|tokens| ASTParser::new(tokens).parse().ok())
                .and_then(|expressions| expressions.into_iter().find_map(from_expression))
        })
    })
}

fn import_modules(source: &str) -> HashSet<String> {
    fn from_expression(expression: Expression) -> Option<String> {
        match expression {
            Expression::List(items) => match items.as_slice() {
                [Expression::Symbol(head), Expression::Symbol(name), ..] if head == "import" => {
                    Some(name.clone())
                }
                _ => None,
            },
            _ => None,
        }
    }

    let mut modules = Parser::new(source.to_string())
        .parse()
        .ok()
        .and_then(|tokens| ASTParser::new(tokens).parse().ok())
        .into_iter()
        .flatten()
        .filter_map(from_expression)
        .collect::<HashSet<_>>();
    // Scratch is an editable source buffer and may temporarily be invalid.
    // Imports inserted by this view are canonical one-line forms, so retain
    // their derived markers/idempotence even while an unrelated form is half
    // typed and prevents a whole-buffer parse.
    for line in source.lines() {
        let Some(expressions) = Parser::new(line.to_string())
            .parse()
            .ok()
            .and_then(|tokens| ASTParser::new(tokens).parse().ok())
        else {
            continue;
        };
        modules.extend(expressions.into_iter().filter_map(from_expression));
    }
    modules
}

fn create_target(root: &Path, input: &str) -> Result<CreateTarget, String> {
    let trimmed = input.trim();
    if trimmed.ends_with('/') {
        let relative = trimmed.trim_end_matches('/');
        if relative.is_empty() || !relative.split('/').all(valid_name_segment) {
            return Err("Folder names may contain letters, numbers, '-', or '_'".to_string());
        }
        return Ok(CreateTarget::Directory(root.join(relative)));
    }

    let input = trimmed.strip_suffix(".lisp").unwrap_or(trimmed);
    let name = if input.contains('.') {
        input.to_string()
    } else {
        format!("my.{input}")
    };
    if !valid_module_name(&name) {
        return Err(
            "Module names use non-empty dotted segments of letters, numbers, '-', or '_'"
                .to_string(),
        );
    }
    let mut path = root.to_path_buf();
    for segment in name.split('.') {
        path.push(segment);
    }
    path.set_extension("lisp");
    Ok(CreateTarget::Module { name, path })
}

fn valid_module_name(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(valid_name_segment)
}

fn valid_name_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn preview_line(root: &Path, query: &str) -> String {
    if query.trim().is_empty() {
        return "Preview: type a module name (for example euclid or my.euclid.sparse)".to_string();
    }
    match create_target(root, query) {
        Ok(CreateTarget::Module { name, path }) => {
            format!("Preview: {}    (module {name})", path.display())
        }
        Ok(CreateTarget::Directory(path)) => format!("Preview: create folder {}/", path.display()),
        Err(error) => format!("Preview: {error}"),
    }
}

fn create_module_file(path: &Path, module: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!("Invalid module path: {}", path.display()));
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Could not create module directory '{}': {error}",
            parent.display()
        )
    })?;
    let source = module_template(module);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Could not create module '{}': {error}", path.display()))?;
    use std::io::Write;
    file.write_all(source.as_bytes())
        .map_err(|error| format!("Could not write module '{}': {error}", path.display()))
}

fn module_template(module: &str) -> String {
    format!(
        "(module {module})\n\n; Attach this module to the current project with C-a in the Packages view.\n; Attach it to every session with C-i, which adds its import to ~/.eseq.d/init.lisp.\n; Modules may register their own UI from their namespace, for example:\n; (effect-buffer \"*my-package*\" (label \"Hello from {module}\"))\n\n(export )\n"
    )
}

#[derive(Debug, Clone, Copy)]
enum AttachmentDestination {
    Scratch,
    UserInit,
}

fn attach_selected_package(
    editor: &mut Editor,
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
    destination: AttachmentDestination,
) {
    let Some(session) = ctx.sessions.package_view_session.as_ref() else {
        return;
    };
    let entries = filtered_entries(session);
    let Some(entry) = entries.get(session.selected.min(entries.len().saturating_sub(1))) else {
        editor.handle_host_event(HostEvent::Status("No package selected".to_string()));
        return;
    };
    let Some(module) = entry.module.as_deref() else {
        editor.handle_host_event(HostEvent::Status(
            if entry.directory {
                "Select a module file to attach"
            } else {
                "The selected file has no (module ...) header"
            }
            .to_string(),
        ));
        return;
    };

    let result = match destination {
        // Attaching to the project loads the module on the spot, the way
        // clicking a script in the Scripts tab used to: the module registers
        // its step tab, effect buffers and macros immediately instead of
        // waiting for the next scratch replay. The scratch line is written
        // only once the module evaluates, so a broken module cannot poison
        // the project.
        AttachmentDestination::Scratch => load_module(editor, module)
            .and_then(|()| attach_to_scratch(editor, module))
            .inspect(|_| record_evaluated_project_import(app, module)),
        AttachmentDestination::UserInit => attach_to_user_init(editor, module),
    };
    match result {
        Ok(already_present) => {
            let destination = match destination {
                AttachmentDestination::Scratch => "project scratch",
                AttachmentDestination::UserInit => "init.lisp",
            };
            let verb = if already_present {
                "Already attached to"
            } else {
                "Attached to"
            };
            editor.handle_host_event(HostEvent::Status(format!("{verb} {destination}: {module}")));
            refresh_packages_view(editor, ctx);
        }
        Err(error) => editor.handle_host_event(HostEvent::Status(error)),
    }
}

/// Evaluate `(import <module>)` in the UI runtime. `import` is load-once and
/// idempotent, so re-attaching an already-loaded module is a no-op.
fn load_module(editor: &mut Editor, module: &str) -> Result<(), String> {
    // Transactional, like eval-buffer: the module's recorded definitions and
    // `override` targets mark the factory effects that read them, so a
    // package that overrides a mixer seam repaints the mixer on attach
    // instead of waiting for an unrelated rerun. A plain `eval_str` here
    // registered the override but left every dependent stale.
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor
        .runtime_mut()
        .eval_source_transactional(None, &format!("(import {module})"), overlays);
    let success = report.success;
    let diagnostics = report.diagnostics.clone();
    editor.process_lisp_reload_report(report);
    if success {
        Ok(())
    } else {
        Err(format!("Could not load '{module}': {}", diagnostics.join("; ")))
    }
}

/// Record an import the host evaluated itself in the project's *evaluated*
/// scratch source.
///
/// A project stores two scratch texts: the draft buffer and the source that
/// was actually evaluated (`ProjectScratchState::evaluated_buffer`). Reopening
/// a project replays the evaluated one, and the scheduler runtime watches it
/// too. Attach runs `(import …)` itself, so without this the line lives only
/// in the draft: the scheduler never sees the module, and the next open
/// replays a scratch that does not mention it — the module silently does not
/// come back.
fn record_evaluated_project_import(app: &mut app::App, module: &str) {
    if let Some(updated) = evaluated_scratch_with_import(&app.state.scratch_source(), module) {
        app.state.set_scratch_source(updated);
    }
}

/// `Some(updated)` when `module` still has to be added to an evaluated scratch
/// source, `None` when it is already there.
fn evaluated_scratch_with_import(evaluated: &str, module: &str) -> Option<String> {
    let (updated, _, already_present) = source_with_import(evaluated, module);
    (!already_present).then_some(updated)
}

fn attach_to_scratch(editor: &mut Editor, module: &str) -> Result<bool, String> {
    let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
    else {
        return Err("Project scratch buffer is not available".to_string());
    };
    let (source, line, already_present) = source_with_import(&buffer.text(), module);
    if !already_present {
        buffer.set_text(&source);
    }
    buffer.cursor = (line, 0);
    editor.mark_needs_redraw();
    Ok(already_present)
}

fn attach_to_user_init(editor: &mut Editor, module: &str) -> Result<bool, String> {
    let path = user_init_path();
    if let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.path.as_ref() == Some(&path))
    {
        let (source, line, already_present) = source_with_import(&buffer.text(), module);
        if !already_present {
            buffer.set_text(&source);
        }
        buffer.cursor = (line, 0);
        editor.mark_needs_redraw();
        return Ok(already_present);
    }

    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Could not read '{}': {error}", path.display())),
    };
    let (source, _, already_present) = source_with_import(&source, module);
    if already_present {
        return Ok(true);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create '{}': {error}", parent.display()))?;
    }
    write_text_atomically(&path, &source)?;
    Ok(false)
}

fn write_text_atomically(path: &Path, source: &str) -> Result<(), String> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid file path: {}", path.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("init.lisp");
    let temporary = parent.join(format!(".{filename}.{}-{stamp}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())?;
        }
        file.write_all(source.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Could not write '{}': {error}", path.display()));
    }
    Ok(())
}

fn source_with_import(source: &str, module: &str) -> (String, usize, bool) {
    if import_modules(source).contains(module) {
        let line = source
            .lines()
            .position(|line| line.contains(module))
            .unwrap_or(0);
        return (source.to_string(), line, true);
    }
    let mut updated = source.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    let line = updated.lines().count();
    updated.push_str(&format!("(import {module})\n"));
    (updated, line, false)
}

fn open_user_init(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let path = user_init_path();
    match editor.open_or_create_file_buffer(&path) {
        Ok(_) => finish_packages_session(editor, ctx),
        Err(error) => editor.handle_host_event(HostEvent::Status(format!(
            "Could not open '{}': {error:?}",
            path.display()
        ))),
    }
}

fn user_init_path() -> PathBuf {
    sequencer::app_paths::app_paths()
        .user_lisp_root()
        .join("init.lisp")
}

fn buffer_source(editor: &Editor, path: &Path, name: &str) -> Option<String> {
    editor
        .buffers
        .iter()
        .find(|buffer| {
            (!name.is_empty() && buffer.name == name)
                || (!path.as_os_str().is_empty()
                    && buffer.path.as_ref() == Some(&path.to_path_buf()))
        })
        .map(|buffer| buffer.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reopening a project replays its *evaluated* scratch, not the draft
    /// buffer, so an import the host evaluated itself has to land there too.
    #[test]
    fn evaluated_scratch_gains_an_attached_import() {
        let updated = evaluated_scratch_with_import("", "demos.graph-variable-reset")
            .expect("an empty evaluated scratch needs the import");
        assert!(
            import_modules(&updated).contains("demos.graph-variable-reset"),
            "attach must be replayable from the evaluated scratch, got {updated:?}"
        );
    }

    #[test]
    fn evaluated_scratch_keeps_existing_lines_and_stays_idempotent() {
        let existing = "(def tempo 120)\n";
        let updated = evaluated_scratch_with_import(existing, "demos.macro-player")
            .expect("a populated evaluated scratch still needs the import");
        assert!(
            updated.contains("(def tempo 120)"),
            "attach must not drop what the project already evaluated, got {updated:?}"
        );
        assert_eq!(
            evaluated_scratch_with_import(&updated, "demos.macro-player"),
            None,
            "re-attaching an already-recorded module must not append a second import"
        );
    }

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "eseq-packages-view-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn bare_and_dotted_names_map_to_module_paths() {
        let root = Path::new("/packages/local");
        assert_eq!(
            create_target(root, "euclid").unwrap(),
            CreateTarget::Module {
                name: "my.euclid".to_string(),
                path: root.join("my/euclid.lisp"),
            }
        );
        assert_eq!(
            create_target(root, "my.euclid.sparse").unwrap(),
            CreateTarget::Module {
                name: "my.euclid.sparse".to_string(),
                path: root.join("my/euclid/sparse.lisp"),
            }
        );
        assert!(create_target(root, "../escape").is_err());
    }

    #[test]
    fn created_module_has_the_packages_template() {
        let root = temp_root("template");
        let path = root.join("my/euclid.lisp");
        create_module_file(&path, "my.euclid").unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(declared_module(&source).as_deref(), Some("my.euclid"));
        assert!(source.contains("(export )"));
        assert!(source.contains("current project"));
        assert!(source.contains("init.lisp"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_attachment_is_derived_and_idempotent() {
        let (source, line, already_present) = source_with_import("(def x 1)\n", "my.euclid");
        assert!(!already_present);
        assert_eq!(line, 2);
        assert!(import_modules(&source).contains("my.euclid"));

        let (second, second_line, already_present) = source_with_import(&source, "my.euclid");
        assert!(already_present);
        assert_eq!(second, source);
        assert_eq!(second_line, line);

        let invalid_scratch = format!("{source}(half-typed");
        assert!(import_modules(&invalid_scratch).contains("my.euclid"));
        let (_, _, already_present) = source_with_import(&invalid_scratch, "my.euclid");
        assert!(
            already_present,
            "an unrelated parse error must not duplicate imports"
        );
    }

    #[test]
    fn scratch_attachment_only_inserts_an_import_line() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
        editor
            .buffers
            .iter_mut()
            .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .expect("default scratch buffer")
            .set_text("(def project-value 1)\n");

        assert!(!attach_to_scratch(&mut editor, "my.euclid").unwrap());
        assert!(attach_to_scratch(&mut editor, "my.euclid").unwrap());
        let source = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .unwrap()
            .text();
        assert_eq!(source.matches("(import my.euclid)").count(), 1);
        assert!(source.contains("(def project-value 1)"));
    }

    #[test]
    fn listing_reads_module_headers_and_ignores_other_files() {
        let root = temp_root("listing");
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("one.lisp"), "(module my.one)\n(half-typed").unwrap();
        std::fs::write(root.join("notes.txt"), "not a module").unwrap();
        let entries = scan_directory(&root).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].directory);
        assert_eq!(entries[1].module.as_deref(), Some("my.one"));
        assert_eq!(exact_entry_index(&entries, "folder"), Some(0));
        assert_eq!(exact_entry_index(&entries, "my.one"), Some(1));
        assert_eq!(exact_entry_index(&entries, "one.lisp"), Some(1));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn staged_package_summary_is_a_dict_until_taken() {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-import-native-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("alec.drums");
        std::fs::create_dir_all(source.join("instruments/kick")).unwrap();
        std::fs::write(source.join("instruments/kick/dsp.lisp"), "(out 0)").unwrap();
        std::fs::write(
            source.join("manifest.json"),
            r#"{"name":"alec/drums","version":"1"}"#,
        )
        .unwrap();
        let packages_dir = root.join("packages");
        let staged =
            sequencer::package_install::stage_package_from_path(&source, &packages_dir).unwrap();
        let staging = staged.staging_path().to_path_buf();
        install_staged(staged);

        let Value::Map(summary) = staged_package_summary_value() else {
            panic!("a staged package summarizes as a dict");
        };
        let get = |key: &str| summary.get(key).map(|value| value.borrow().clone());
        assert_eq!(get("identity"), Some(Value::String("alec/drums".into())));
        assert_eq!(get("instruments"), Some(Value::Number(1.0)));
        assert_eq!(get("effects"), Some(Value::Number(0.0)));
        assert_eq!(get("installed?"), Some(Value::Bool(false)));
        assert_eq!(
            get("path"),
            Some(Value::String(packages_dir.join("alec.drums").display().to_string()))
        );

        // Cancel discards the staging directory and empties the summary.
        let staged = take_staged().expect("still staged");
        sequencer::package_install::discard_staged_package(staged);
        assert!(!staging.exists());
        assert_eq!(staged_package_summary_value(), Value::Nil);
        std::fs::remove_dir_all(root).unwrap();
    }
}
