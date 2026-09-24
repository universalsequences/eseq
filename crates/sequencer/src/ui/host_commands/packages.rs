use crate::*;
use sequencer::app::{
    import_modules, source_forms, source_with_import, source_with_leading_import,
    source_without_import,
};

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
    // Path-keyed package commands shared by the Packages tab and the text
    // view (eseq-mods.18.1). Every one takes a `:path` (a module file) or a
    // `:module` name; attach/detach edit the textual import record only.
    "packages-attach",
    "packages-detach",
    "packages-always-load",
    "packages-stop-always-load",
    "packages-open-source",
    "packages-close-source",
    "packages-open-init",
    "packages-open-scratch",
    "packages-create",
    "packages-copy-to-local",
    "packages-refresh",
    // Instance kinds (docs/instance-kinds-spec.md §8.3): `{:module :kind
    // [:group-id]}` attaches the module to the project when it is not yet,
    // then creates an instance of the kind (rack-owned with :group-id).
    "packages-new-instance",
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
        "packages-new-instance" => {
            new_instance_from_package(&payload, app, editor, ctx);
        }
        "packages-detach" if !module_instance_ids_from_payload(app, &payload).is_empty() => {
            if detach_with_instances_command(&payload, app, editor) {
                refresh_package_listings(editor, ctx);
            }
        }
        "packages-attach" | "packages-detach" | "packages-always-load"
        | "packages-stop-always-load" => {
            let result = payload_module(&payload).and_then(|module| {
                let (destination, attach) = match name {
                    "packages-attach" => (AttachmentDestination::Scratch, true),
                    "packages-detach" => (AttachmentDestination::Scratch, false),
                    "packages-always-load" => (AttachmentDestination::UserInit, true),
                    _ => (AttachmentDestination::UserInit, false),
                };
                if attach {
                    attach_module(editor, app, &module, destination).map(|(already, warnings)| {
                        with_warnings(attachment_status(&module, destination, already), &warnings)
                    })
                } else {
                    detach_module(editor, app, &module, destination)
                        .map(|removed| detachment_status(&module, destination, removed))
                }
            });
            match result {
                Ok(message) => editor.show_transient_message(message),
                Err(error) => editor.show_transient_message(error),
            }
            refresh_package_listings(editor, ctx);
        }
        "packages-open-source" => {
            let result = (|| -> Result<String, String> {
                let path = extract_string_from_payload(&payload, "path")
                    .filter(|path| !path.trim().is_empty())
                    .ok_or("Expected a :path")?;
                let path = PathBuf::from(path);
                let label = extract_string_from_payload(&payload, "label")
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or_else(|| source_tab_label(&path));
                let read_only = extract_bool_from_payload(&payload, "read-only");
                if ctx.sessions.package_view_session.is_some() {
                    close_packages_view(editor, ctx);
                }
                open_source_tab(editor, &path, &label, read_only)?;
                Ok(format!("Opened {label}"))
            })();
            match result {
                Ok(message) => editor.show_transient_message(message),
                Err(error) => editor.show_transient_message(error),
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "packages-close-source" => {
            let Some(buffer) = extract_string_from_payload(&payload, "buffer")
                .filter(|buffer| !buffer.trim().is_empty())
            else {
                return;
            };
            close_source_tab(editor, &buffer);
        }
        "packages-open-init" => {
            if ctx.sessions.package_view_session.is_some() {
                close_packages_view(editor, ctx);
            }
            if let Err(error) = open_user_init_tab(editor) {
                editor.show_transient_message(error);
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "packages-open-scratch" => {
            if ctx.sessions.package_view_session.is_some() {
                close_packages_view(editor, ctx);
            }
            if let Err(error) = register_source_tab(editor, "scratch", PROJECT_SCRATCH_BUFFER_NAME) {
                editor.show_transient_message(error);
            }
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "packages-create" => {
            let result = (|| -> Result<String, String> {
                let name = extract_string_from_payload(&payload, "name")
                    .filter(|name| !name.trim().is_empty())
                    .ok_or("Type a module name (for example euclid or my.euclid.sparse)")?;
                let root = sequencer::app_paths::app_paths().local_modules_dir();
                match create_target(&root, &name)? {
                    CreateTarget::Module { name, path } => {
                        if path.exists() {
                            return Err(format!("{name} already exists"));
                        }
                        create_module_file(&path, &name)?;
                        if ctx.sessions.package_view_session.is_some() {
                            close_packages_view(editor, ctx);
                        }
                        open_source_tab(editor, &path, &source_tab_label(&path), false)?;
                        Ok(format!("Created {name}"))
                    }
                    CreateTarget::Directory(path) => {
                        std::fs::create_dir_all(&path).map_err(|error| {
                            format!("Could not create folder '{}': {error}", path.display())
                        })?;
                        Ok(format!("Created folder {}/", path.display()))
                    }
                }
            })();
            match result {
                Ok(message) => editor.show_transient_message(message),
                Err(error) => editor.show_transient_message(error),
            }
            refresh_package_listings(editor, ctx);
        }
        "packages-copy-to-local" => {
            let result = (|| -> Result<String, String> {
                let path = extract_string_from_payload(&payload, "path")
                    .filter(|path| !path.trim().is_empty())
                    .ok_or("Expected a :path")?;
                let source = PathBuf::from(path);
                let module = module_at_path(&source)?;
                let root = sequencer::app_paths::app_paths().local_modules_dir();
                let target = copy_module_to_local(&source, &module, &root)?;
                if ctx.sessions.package_view_session.is_some() {
                    close_packages_view(editor, ctx);
                }
                open_source_tab(editor, &target, &source_tab_label(&target), false)?;
                Ok(format!("Copied {module} to Local (it now shadows the package copy)"))
            })();
            match result {
                Ok(message) => editor.show_transient_message(message),
                Err(error) => editor.show_transient_message(error),
            }
            refresh_package_listings(editor, ctx);
        }
        "packages-refresh" => {
            sequencer::app_paths::invalidate_package_catalog_cache();
            refresh_package_listings(editor, ctx);
        }
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
    // The text view took over the sequencer's tile; give it back first so
    // the source tab lands in the step-tab bar the way it does from the
    // Packages tab.
    close_packages_view(editor, ctx);
    if let Err(error) = open_source_tab(editor, path, &source_tab_label(path), false) {
        editor.handle_host_event(HostEvent::Status(error));
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
    use eseqlisp::parser::ExprKind;
    source_forms(source).into_iter().find_map(|form| {
        let ExprKind::List(items) = form.kind else { return None };
        match items.as_slice() {
            [head, name, ..] if matches!(&head.kind, ExprKind::Symbol(s) if s == "module") => {
                match &name.kind {
                    ExprKind::Symbol(name) => Some(name.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    })
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
        "(module {module})\n\n; Attach this module to the current project from the browser's Packages tab\n; (Enter, or right-click > Attach to Project), or with C-a in the C-x p text view.\n; Right-click > Always Load (C-i in the text view) adds its import to ~/.eseq.d/init.lisp.\n; Modules may register their own UI from their namespace, for example:\n; (effect-buffer \"*my-package*\" (label \"Hello from {module}\"))\n\n(export )\n"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachmentDestination {
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

    let result = attach_module(editor, app, module, destination);
    match result {
        Ok((already_present, warnings)) => {
            let message =
                with_warnings(attachment_status(module, destination, already_present), &warnings);
            editor.handle_host_event(HostEvent::Status(message));
            refresh_packages_view(editor, ctx);
        }
        Err(error) => editor.handle_host_event(HostEvent::Status(error)),
    }
}

/// The ids of every project instance of a kind `module` defines (its
/// manifest `kinds` plus registered `def-kind`s), whatever owns it.
pub(crate) fn module_instance_ids(app: &app::App, module: &str) -> Vec<u64> {
    let catalog = sequencer::app_paths::app_paths().package_catalog();
    let kinds: Vec<String> = module_kinds(&catalog, &registered_tree_kinds())
        .into_iter()
        .filter(|kind| kind.module == module)
        .map(|kind| kind.id)
        .collect();
    instance_ids_of_kinds(app, &kinds)
}

fn instance_ids_of_kinds(app: &app::App, kinds: &[String]) -> Vec<u64> {
    app.instances
        .list
        .iter()
        .filter(|instance| kinds.contains(&instance.kind))
        .map(|instance| instance.id)
        .collect()
}

fn module_instance_ids_from_payload(app: &app::App, payload: &Value) -> Vec<u64> {
    payload_module(payload).map(|module| module_instance_ids(app, &module)).unwrap_or_default()
}

/// `packages-detach` of a module whose kinds have instances. Detaching
/// deletes them, so the first request only asks (spec §8.3): it opens
/// `eseq.file-dialogs/open-confirm`, whose Continue re-sends the command
/// with `:confirmed true`, and that rerun detaches. Returns true once it
/// detached (the package listings then need a refresh).
pub(crate) fn detach_with_instances_command(
    payload: &Value,
    app: &mut app::App,
    editor: &mut Editor,
) -> bool {
    let module = payload_module(payload).unwrap_or_default();
    if !extract_bool_from_payload(payload, "confirmed") {
        let count = module_instance_ids(app, &module).len();
        let package = sequencer::lisp_host::package_name_for_module(&module)
            .unwrap_or_else(|| module.clone());
        let message = detach_confirm_message(&package, count);
        super::file_menu::activate_dialog_tile(editor);
        let form = format!(
            "(eseq.file-dialogs/open-confirm {} (lambda () (host-command \"packages-detach\" (dict :module {} :confirmed true))))",
            super::file_menu::lisp_string(&message),
            super::file_menu::lisp_string(&module),
        );
        if let Err(error) = editor.runtime_mut().eval_str(&form) {
            editor.show_transient_message(format!("Could not ask to detach {module}: {error:?}"));
        }
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
        return false;
    }
    let result = detach_module_with_instances(editor, app, &module);
    super::instances::sync_instances_to_editor(app, editor);
    match result {
        Ok(message) => editor.show_transient_message(message),
        Err(error) => editor.show_transient_message(error),
    }
    true
}

/// "Detach alez/neural and delete its 3 instances?"
pub(crate) fn detach_confirm_message(package: &str, count: usize) -> String {
    if count == 1 {
        format!("Detach {package} and delete its instance?")
    } else {
        format!("Detach {package} and delete its {count} instances?")
    }
}

/// Detach `module` from the project together with every instance of its
/// kinds, as ONE undoable edit: the instances (and their overrides) and the
/// evaluated scratch's import line go in one `delete_instances_recorded`,
/// so one undo brings the instances back AND re-records the import (the
/// module stays loaded this session anyway: `import` cannot unload, so the
/// kind is still registered and the instances revive at once). The draft
/// scratch buffer and the module's `override` toggle live outside history;
/// [`apply_replayed_scratch_imports`] mirrors undo/redo into both.
pub(crate) fn detach_module_with_instances(
    editor: &mut Editor,
    app: &mut app::App,
    module: &str,
) -> Result<String, String> {
    let ids = module_instance_ids(app, module);
    let removed_from_evaluated = import_modules(&app.state.scratch_source()).contains(module);
    let removed = app.delete_instances_recorded(&ids, "Detach package", Some(module))?;
    // The evaluated import is gone already, so this reports (and switches
    // the overrides off for) the draft line only.
    let removed_line = detach_module(editor, app, module, AttachmentDestination::Scratch)?;
    if removed_from_evaluated
        && !removed_line
        && !module_still_attached_elsewhere(&app.state, module, AttachmentDestination::Scratch)
    {
        set_module_overrides_enabled(editor, module, false);
    }
    let mut status = detachment_status(module, AttachmentDestination::Scratch, removed_line || removed_from_evaluated);
    if !removed.is_empty() {
        let noun = if removed.len() == 1 { "instance" } else { "instances" };
        status.push_str(&format!("; deleted {} {noun}", removed.len()));
    }
    Ok(status)
}

/// After an undo/redo that re-added or re-removed a module's evaluated
/// scratch import (detach with instances), bring the parts history does not
/// hold along: the draft scratch buffer's line (else the next scratch
/// evaluation would overwrite the evaluated scratch with a draft that
/// disagrees) and the module's `override` entries.
pub(crate) fn apply_replayed_scratch_imports(
    editor: &mut Editor,
    state: &SequencerState,
    imports: &[app::history::ScratchImportState],
) {
    for import in imports {
        let module = import.module.as_str();
        let Some(buffer) = editor
            .buffers
            .iter_mut()
            .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
        else {
            continue;
        };
        let draft = buffer.text();
        if import.present {
            if let Some(updated) = source_with_leading_import(&draft, module) {
                buffer.set_text(&updated);
            }
            set_module_overrides_enabled(editor, module, true);
        } else {
            let (updated, removed) = source_without_import(&draft, module);
            if removed {
                buffer.set_text(&updated);
            }
            if !module_still_attached_elsewhere(state, module, AttachmentDestination::Scratch) {
                set_module_overrides_enabled(editor, module, false);
            }
        }
        editor.mark_needs_redraw();
    }
}

/// `packages-new-instance {:module :kind [:group-id]}`: "New <kind>" from a
/// module row's menu, the double-click that creates a module's first
/// instance, and the rack menu's kind picker. Attaches the module to the
/// project first when it is not (the attach loads it, which registers the
/// kind), then creates the instance through `instance-create`, which
/// syncs the records and opens the new instance's tab.
fn new_instance_from_package(
    payload: &Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = (|| -> Result<Option<String>, String> {
        let kind = extract_string_from_payload(payload, "kind")
            .map(|kind| kind.trim().to_string())
            .filter(|kind| !kind.is_empty())
            .ok_or("packages-new-instance needs a :kind")?;
        let module = extract_string_from_payload(payload, "module")
            .map(|module| module.trim().to_string())
            .filter(|module| !module.is_empty());
        let mut warnings = None;
        if let Some(module) = &module {
            if !import_modules(&app.state.scratch_source()).contains(module) {
                let (_, attach_warnings) =
                    attach_module(editor, app, module, AttachmentDestination::Scratch)?;
                if !attach_warnings.is_empty() {
                    warnings = Some(with_warnings(format!("Attached {module}"), &attach_warnings));
                }
            }
        }
        if sequencer::lisp_host::registered_kind(&kind).is_none() {
            return Err(match &module {
                Some(module) => format!("{module} did not define kind {kind}"),
                None => format!("Kind {kind} is not loaded; attach its package first"),
            });
        }
        Ok(warnings)
    })();
    match result {
        Ok(warnings) => {
            let mut fields = vec![(
                "kind",
                Value::String(extract_string_from_payload(payload, "kind").unwrap_or_default().trim().to_string()),
            )];
            if let Some(group_id) = extract_usize_from_payload(payload, "group-id") {
                fields.push(("group-id", Value::Number(group_id as f64)));
            }
            super::instances::handle(
                "instance-create",
                crate::values::map_value(fields),
                app,
                editor,
                ctx,
            );
            if let Some(warnings) = warnings {
                editor.show_transient_message(warnings);
            }
        }
        Err(error) => editor.show_transient_message(error),
    }
    refresh_package_listings(editor, ctx);
}

/// Append attach warnings (e.g. a `def-kind` missing from the manifest) to a
/// status line.
fn with_warnings(status: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        status
    } else {
        format!("{status} (warning: {})", warnings.join("; "))
    }
}

/// Check the attached module's kinds against its package manifest
/// (instance-kinds spec §8.1): a declared kind the module never `def-kind`s
/// is an error, an undeclared `def-kind` a warning. Modules outside any
/// installed package have no manifest to check.
pub(crate) fn check_attached_module_kinds(module: &str) -> Result<Vec<String>, String> {
    let catalog = sequencer::app_paths::app_paths().package_catalog();
    let Some(package) = catalog.package_for_module(module) else {
        return Ok(Vec::new());
    };
    let warnings = sequencer::lisp_host::check_manifest_kinds(&package.manifest, module)?;
    for warning in &warnings {
        eprintln!("metal_seq: {warning}");
    }
    Ok(warnings)
}

/// Attach `module` to one destination. `Ok((true, _))` when it was already
/// there; the second part lists attach warnings.
///
/// Attaching to the project loads the module on the spot, the way clicking
/// a script in the Scripts tab used to: the module registers its step tab,
/// effect buffers and macros immediately instead of waiting for the next
/// scratch replay. The scratch line is written only once the module
/// evaluates, so a broken module cannot poison the project.
pub(crate) fn attach_module(
    editor: &mut Editor,
    app: &mut app::App,
    module: &str,
    destination: AttachmentDestination,
) -> Result<(bool, Vec<String>), String> {
    let result = match destination {
        AttachmentDestination::Scratch => {
            attach_module_to_scratch(editor, app, module, check_attached_module_kinds)
        }
        AttachmentDestination::UserInit => {
            attach_to_user_init_at(editor, &user_init_path(), module).map(|already| (already, Vec::new()))
        }
    };
    if result.is_ok() {
        // A module detached earlier this session had its overrides switched
        // off; attaching it again switches them back on (idempotent).
        set_module_overrides_enabled(editor, module, true);
    }
    result
}

/// The scratch half of [`attach_module`]: load the module, check its kinds,
/// and only then write the import line. `check_kinds` is the manifest check
/// (split out so tests can supply a manifest without an installed package).
/// A failed check writes nothing and forgets the kinds the module just
/// registered with the host, so `instance-create` cannot instantiate a kind
/// whose attach was refused.
fn attach_module_to_scratch(
    editor: &mut Editor,
    app: &mut app::App,
    module: &str,
    check_kinds: impl FnOnce(&str) -> Result<Vec<String>, String>,
) -> Result<(bool, Vec<String>), String> {
    load_module(editor, module)?;
    let warnings = match check_kinds(module) {
        Ok(warnings) => warnings,
        Err(error) => {
            sequencer::lisp_host::unregister_module_kinds(module);
            return Err(error);
        }
    };
    let already = attach_to_scratch(editor, module)?;
    record_evaluated_project_import(app, module);
    Ok((already, warnings))
}

/// Switch a module's `override` entries on or off in the UI runtime, the
/// same per-module toggle the Customize modal offers. `import` is load-once
/// and nothing unloads, so this is how detach takes effect immediately: the
/// factory seams the module replaced are back on screen as soon as the
/// import line is gone. The dependents of every touched target are marked
/// stale by the VM, so a mixer replacement repaints on the spot.
fn set_module_overrides_enabled(editor: &mut Editor, module: &str, enabled: bool) {
    let form = if enabled {
        format!("(enable-module-overrides {module})")
    } else {
        format!("(disable-module-overrides {module})")
    };
    if let Err(error) = editor.runtime_mut().eval_str(&form) {
        eprintln!("metal_seq: could not toggle overrides for {module}: {error:?}");
    }
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

/// True when `module` is still imported by the other attachment record: a
/// module removed from the project but kept in init.lisp (or the reverse)
/// stays live, so its overrides must stay on.
fn module_still_attached_elsewhere(state: &SequencerState, module: &str, removed_from: AttachmentDestination) -> bool {
    match removed_from {
        AttachmentDestination::Scratch => {
            let init = std::fs::read_to_string(user_init_path()).unwrap_or_default();
            import_modules(&init).contains(module)
        }
        AttachmentDestination::UserInit => import_modules(&state.scratch_source()).contains(module),
    }
}

/// Remove `module`'s import from one destination. `Ok(true)` when a line was
/// removed. `import` is load-once and there is no unload, so the module's
/// plain definitions stay in this session; its `override` entries are
/// switched off (unless the other record still imports it), which is what
/// the user sees. The next project open, and the scheduler runtime that
/// rebuilds from the evaluated scratch on every version, no longer load it.
pub(crate) fn detach_module(
    editor: &mut Editor,
    app: &mut app::App,
    module: &str,
    destination: AttachmentDestination,
) -> Result<bool, String> {
    let removed = match destination {
        AttachmentDestination::Scratch => {
            detach_from_project(editor, &app.state, module)?
        }
        AttachmentDestination::UserInit => {
            detach_from_user_init_at(editor, &user_init_path(), module)?
        }
    };
    if removed && !module_still_attached_elsewhere(&app.state, module, destination) {
        set_module_overrides_enabled(editor, module, false);
    }
    Ok(removed)
}

fn destination_name(destination: AttachmentDestination) -> &'static str {
    match destination {
        AttachmentDestination::Scratch => "project",
        AttachmentDestination::UserInit => "every session (init.lisp)",
    }
}

fn attachment_status(module: &str, destination: AttachmentDestination, already: bool) -> String {
    let verb = if already { "Already attached to" } else { "Attached to" };
    format!("{verb} {}: {module}", destination_name(destination))
}

fn detachment_status(module: &str, destination: AttachmentDestination, removed: bool) -> String {
    if removed {
        format!(
            "Removed from {}: {module}",
            destination_name(destination)
        )
    } else {
        format!("{module} was not attached to {}", destination_name(destination))
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

/// Persist only the requested import edit, then mirror it into an open draft.
/// Saving the entire open buffer here would also save unrelated user edits.
fn attach_to_user_init_at(editor: &mut Editor, path: &Path, module: &str) -> Result<bool, String> {
    let persisted = read_user_init(path)?;
    let (source, _, already_present) = source_with_import(&persisted, module);
    if !already_present {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create '{}': {error}", parent.display()))?;
        }
        write_text_atomically(path, &source)?;
    }
    if let Some(buffer) = editor.buffers.iter_mut().find(|buffer| buffer.path.as_deref() == Some(path)) {
        let (draft, line, present_in_draft) = source_with_import(&buffer.text(), module);
        if !present_in_draft {
            buffer.set_text(&draft);
        }
        buffer.dirty = draft != source;
        buffer.cursor = (line, 0);
        editor.mark_needs_redraw();
    }
    Ok(already_present)
}

fn read_user_init(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("Could not read '{}': {error}", path.display())),
    }
}

fn detach_from_scratch(editor: &mut Editor, module: &str) -> Result<bool, String> {
    let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
    else {
        return Err("Project scratch buffer is not available".to_string());
    };
    let (source, removed) = source_without_import(&buffer.text(), module);
    if removed {
        buffer.set_text(&source);
        editor.mark_needs_redraw();
    }
    Ok(removed)
}

fn detach_from_project(editor: &mut Editor, state: &SequencerState, module: &str) -> Result<bool, String> {
    let draft_removed = detach_from_scratch(editor, module)?;
    let (updated, evaluated_removed) = source_without_import(&state.scratch_source(), module);
    if evaluated_removed {
        state.set_scratch_source(updated);
    }
    Ok(draft_removed || evaluated_removed)
}

fn detach_from_user_init_at(editor: &mut Editor, path: &Path, module: &str) -> Result<bool, String> {
    let (source, removed) = source_without_import(&read_user_init(path)?, module);
    if removed {
        write_text_atomically(path, &source)?;
    }
    let mut draft_removed = false;
    if let Some(buffer) = editor.buffers.iter_mut().find(|buffer| buffer.path.as_deref() == Some(path)) {
        let (draft, removed) = source_without_import(&buffer.text(), module);
        draft_removed = removed;
        if removed {
            buffer.set_text(&draft);
        }
        buffer.dirty = draft != source;
        editor.mark_needs_redraw();
    }
    Ok(removed || draft_removed)
}

pub(super) fn write_text_atomically(path: &Path, source: &str) -> Result<(), String> {
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

fn open_user_init(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    close_packages_view(editor, ctx);
    if let Err(error) = open_user_init_tab(editor) {
        editor.handle_host_event(HostEvent::Status(error));
    }
}

fn open_user_init_tab(editor: &mut Editor) -> Result<String, String> {
    let path = user_init_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create '{}': {error}", parent.display()))?;
        }
        write_text_atomically(&path, "")?;
    }
    open_source_tab(editor, &path, "init.lisp", false)
}

pub(super) fn user_init_path() -> PathBuf {
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


// ── Source tabs ──
//
// "View source" opens a module file as a closable tab in the sequencer
// tile. The tab is a plain view onto the file: its × only unregisters the
// tab and drops the buffer (`packages-close-source`), never touching the
// import record. Attach and detach live only in the Packages tab's context
// menu and the text view's keys. The same path serves init.lisp and the
// project scratch from the File menu.

fn source_tab_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("source")
        .to_string()
}

/// Open `path` in a buffer and register it as a closable source tab, then
/// select it. Returns the buffer name. A read-only tab is for installed and
/// factory sources, which an import would overwrite.
pub(crate) fn open_source_tab(
    editor: &mut Editor,
    path: &Path,
    label: &str,
    read_only: bool,
) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("'{}' is not a file", path.display()));
    }
    let buffer_name = crate::edit_sessions::open_script_source_buffer(editor, path)?;
    if let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.name == buffer_name)
    {
        buffer.read_only = read_only;
    }
    register_source_tab(editor, label, &buffer_name)?;
    Ok(buffer_name)
}

/// Register `buffer` as a closable source tab in the sequencer tile and
/// switch to it.
fn register_source_tab(editor: &mut Editor, label: &str, buffer: &str) -> Result<String, String> {
    let label_literal = crate::edit_sessions::escape_lisp_string(label);
    let buffer_literal = crate::edit_sessions::escape_lisp_string(buffer);
    editor
        .runtime_mut()
        .eval_str(&format!(
            "(eseq.seq-step-tabs/seq-register-source-tab \"{label_literal}\" \"{buffer_literal}\")"
        ))
        .map_err(|error| format!("Could not register source tab: {error:?}"))?;
    let _ = editor.runtime_mut().eval_str(&format!(
        "(eseq.seq-step-tabs/seq-select-main-step-tab-by-buffer \"{buffer_literal}\")"
    ));
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    Ok(buffer.to_string())
}

/// The tab's × handler: unregister the tab and drop the buffer. The project
/// scratch is never dropped, and a buffer with unsaved edits is kept (with
/// its tab gone) rather than silently saved or discarded.
fn close_source_tab(editor: &mut Editor, buffer: &str) {
    let buffer_literal = crate::edit_sessions::escape_lisp_string(buffer);
    let _ = editor.runtime_mut().eval_str(&format!(
        "(eseq.seq-step-tabs/seq-unregister-step-sequencer-tab \"{buffer_literal}\")"
    ));
    editor.refresh_runtime_side_effects();
    if buffer != PROJECT_SCRATCH_BUFFER_NAME {
        let dirty = editor
            .buffers
            .iter()
            .find(|candidate| candidate.name == buffer)
            .is_some_and(|candidate| candidate.dirty);
        if dirty {
            editor.show_transient_message(format!(
                "{buffer} has unsaved changes; the buffer is kept open"
            ));
        } else {
            editor.remove_buffer_by_name(buffer);
        }
    }
    editor.mark_needs_redraw();
}

fn payload_module(payload: &Value) -> Result<String, String> {
    if let Some(module) =
        extract_string_from_payload(payload, "module").filter(|module| !module.trim().is_empty())
    {
        return Ok(module.trim().to_string());
    }
    let path = extract_string_from_payload(payload, "path")
        .filter(|path| !path.trim().is_empty())
        .ok_or("Expected a :module name or a :path")?;
    module_at_path(Path::new(&path))
}

/// The `(module …)` a file declares, or why it cannot be attached.
fn module_at_path(path: &Path) -> Result<String, String> {
    if path.is_dir() {
        return Err("Select a module file to attach, not a folder".to_string());
    }
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("Could not read '{}': {error}", path.display()))?;
    declared_module(&source)
        .ok_or_else(|| format!("'{}' has no (module ...) header", source_tab_label(path)))
}

/// Copy a read-only (installed or factory) module into the Local workspace
/// under the path its module name maps to. Local is the first module load
/// root, so the copy shadows the package's file under the same name: the
/// user edits their copy and every `(import …)` keeps working.
fn copy_module_to_local(source: &Path, module: &str, local_root: &Path) -> Result<PathBuf, String> {
    if !valid_module_name(module) {
        return Err(format!("'{module}' is not a valid module name"));
    }
    let mut target = local_root.to_path_buf();
    for segment in module.split('.') {
        target.push(segment);
    }
    target.set_extension("lisp");
    if target.exists() {
        return Err(format!("Local already has {module} ({})", target.display()));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create '{}': {error}", parent.display()))?;
    }
    std::fs::copy(source, &target).map_err(|error| {
        format!("Could not copy '{}' to '{}': {error}", source.display(), target.display())
    })?;
    Ok(target)
}

/// Re-list every package surface after the import record or the package
/// set changed: the text view if it is open, and the browser tab.
fn refresh_package_listings(editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    if ctx.sessions.package_view_session.is_some() {
        refresh_packages_view(editor, ctx);
    }
    let _ = editor.runtime_mut().eval_str("(eseq.browser/refresh-buffer)");
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

// ── Packages tab tree ──
//
// `(seq-package-tree query)` lists everything importable as one tree with
// three roots: Local (`~/.eseq.d/packages/local`, manifest-free personal
// modules), Installed (packages under `~/.eseq.d/packages` with a
// manifest), and Factory (packages shipped with the application). Items
// carry the module name and attachment marks so the tab can act on them
// without a second lookup; the record they reflect is the evaluated project
// scratch plus `~/.eseq.d/init.lisp`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageTreeNode {
    label: String,
    /// "root" | "folder" | "package" | "module" | "file"
    kind: &'static str,
    /// "local" | "installed" | "factory"
    tier: &'static str,
    path: Option<PathBuf>,
    module: Option<String>,
    detail: Option<String>,
    read_only: bool,
    /// Imported by the evaluated project scratch.
    attached: bool,
    /// Imported by `~/.eseq.d/init.lisp`.
    always: bool,
    children: Vec<PackageTreeNode>,
    /// Instance kinds this module defines (manifest `kinds` plus any
    /// `def-kind` the registry saw), instance-kinds spec §8.2.
    kinds: Vec<TreeKind>,
    /// Instances of those kinds, whatever owns them (the count badge).
    instance_count: usize,
    /// Set on "instance" rows only.
    instance: Option<TreeInstance>,
}

/// One kind a module row offers as `New <kind>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeKind {
    /// `<package>:<kind>`.
    pub id: String,
    /// The authored kind name.
    pub name: String,
    /// The module that defines it.
    pub module: String,
}

/// One project instance as the Packages tab lists it (read from
/// `SEQ.instances`, which the reactive tick publishes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeInstance {
    pub id: u64,
    pub kind: String,
    pub label: String,
    /// "project" or the owning rack's name.
    pub owner: String,
    pub owner_rack: Option<u64>,
    /// Whether the kind is registered in this session. An unregistered
    /// kind's instance is a placeholder (spec §5): kept, not published.
    pub registered: bool,
}

pub(crate) fn register_package_tree_natives(runtime: &mut Runtime, state: Arc<SequencerState>) {
    runtime.register_native_with_docs(
        "seq-package-tree",
        "(seq-package-tree query [instances])",
        "Return the Packages browser tree (Local / Installed / Factory roots) filtered by query. \
         `instances` is `SEQ.instances`: module rows that define kinds get a count badge and one \
         child row per instance.",
        move |args, _ctx| {
            let query = match args.first() {
                Some(Value::String(query)) => query.as_str(),
                _ => "",
            };
            let instances = args.get(1).map(tree_instances_from_value).unwrap_or_default();
            let app_paths = sequencer::app_paths::app_paths();
            let catalog = app_paths.package_catalog();
            let init = std::fs::read_to_string(user_init_path()).unwrap_or_default();
            let mut tree = build_package_tree(
                &app_paths.local_modules_dir(),
                &catalog,
                &app_paths.packages_dir(),
                &state.scratch_source(),
                &init,
            );
            let kinds = module_kinds(&catalog, &registered_tree_kinds());
            annotate_package_kinds(&mut tree, &kinds, &instances);
            Ok(package_tree_to_value(&filter_package_tree(&tree, &query.trim().to_lowercase())))
        },
    );
    runtime.register_native_with_docs(
        "seq-instance-kinds",
        "(seq-instance-kinds)",
        "Every kind an instance can be created from: each installed package's manifest `kinds`, \
         then registered kinds no manifest declares. Each is a dict with :id :name :module \
         (nil for project code) and :registered?.",
        move |_args, _ctx| {
            let catalog = sequencer::app_paths::app_paths().package_catalog();
            let registered = sequencer::lisp_host::registered_kinds();
            let mut kinds = module_kinds(&catalog, &registered_tree_kinds());
            // Project-code kinds (no module) can be instantiated too.
            for kind in &registered {
                if kind.module.is_none() && !kinds.iter().any(|known| known.id == kind.id) {
                    kinds.push(TreeKind { id: kind.id.clone(), name: kind.name.clone(), module: String::new() });
                }
            }
            Ok(Value::List(
                kinds
                    .into_iter()
                    .map(|kind| {
                        let registered = registered.iter().any(|known| known.id == kind.id);
                        Rc::new(RefCell::new(crate::values::map_value(vec![
                            ("id", Value::String(kind.id)),
                            ("name", Value::String(kind.name)),
                            (
                                "module",
                                if kind.module.is_empty() { Value::Nil } else { Value::String(kind.module) },
                            ),
                            ("registered?", Value::Bool(registered)),
                        ])))
                    })
                    .collect(),
            ))
        },
    );
}

/// Registered kinds that belong to a module, as tree kinds.
fn registered_tree_kinds() -> Vec<TreeKind> {
    sequencer::lisp_host::registered_kinds()
        .into_iter()
        .filter_map(|kind| {
            Some(TreeKind { module: kind.module?, id: kind.id, name: kind.name })
        })
        .collect()
}

/// Every module's kinds: the manifest `kinds` of each installed package
/// (so `New <kind>` is offered before anything is evaluated, spec §8.1),
/// then any registered kind a manifest does not declare.
pub(crate) fn module_kinds(
    catalog: &eseqlisp::package::PackageCatalog,
    registered: &[TreeKind],
) -> Vec<TreeKind> {
    let mut kinds: Vec<TreeKind> = catalog
        .ordered()
        .flat_map(|package| {
            package.manifest.kinds.iter().map(|kind| TreeKind {
                id: package.manifest.kind_id(&kind.name),
                name: kind.name.clone(),
                module: kind.module.clone(),
            })
        })
        .collect();
    for kind in registered {
        if !kinds.iter().any(|known| known.id == kind.id) {
            kinds.push(kind.clone());
        }
    }
    kinds
}

/// Parse `SEQ.instances` (see `build_instances_value`).
pub(crate) fn tree_instances_from_value(value: &Value) -> Vec<TreeInstance> {
    let Value::List(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let Value::Map(map) = &*item.borrow() else {
                return None;
            };
            let field = |key: &str| map.get(key).map(|value| value.borrow().clone());
            let text = |key: &str| match field(key) {
                Some(Value::String(text)) => Some(text),
                _ => None,
            };
            let id = match field("id") {
                Some(Value::Number(id)) if id >= 0.0 => id as u64,
                _ => return None,
            };
            Some(TreeInstance {
                id,
                kind: text("kind")?,
                label: text("label").unwrap_or_else(|| format!("instance {id}")),
                owner: text("owner-label").unwrap_or_else(|| "project".to_string()),
                owner_rack: match field("owner-rack") {
                    Some(Value::Number(group_id)) if group_id >= 0.0 => Some(group_id as u64),
                    _ => None,
                },
                registered: !matches!(field("registered?"), Some(Value::Bool(false))),
            })
        })
        .collect()
}

fn instance_tree_node(instance: &TreeInstance) -> PackageTreeNode {
    let detail = if instance.registered {
        instance.owner.clone()
    } else {
        format!("{} · not loaded", instance.owner)
    };
    PackageTreeNode {
        label: instance.label.clone(),
        kind: "instance",
        tier: "loaded",
        path: None,
        module: None,
        detail: Some(detail),
        read_only: true,
        attached: false,
        always: false,
        children: Vec::new(),
        kinds: Vec::new(),
        instance_count: 0,
        instance: Some(instance.clone()),
    }
}

/// Instance-kinds spec §8.2: every module row that defines kinds lists
/// them, counts every instance of them (any owner) for its badge, and
/// expands to one row per instance showing the owner. Instances no module
/// row claims (their package is missing, or the kind lives in project code)
/// are grouped per kind at the end of the Loaded section, so a placeholder
/// is visible and can still be deleted.
pub(crate) fn annotate_package_kinds(
    roots: &mut [PackageTreeNode],
    kinds: &[TreeKind],
    instances: &[TreeInstance],
) {
    fn walk(nodes: &mut [PackageTreeNode], kinds: &[TreeKind], instances: &[TreeInstance]) {
        for node in nodes {
            if let (true, Some(module)) = (node.kind == "module", node.module.as_ref()) {
                node.kinds =
                    kinds.iter().filter(|kind| &kind.module == module).cloned().collect();
                let rows: Vec<PackageTreeNode> = instances
                    .iter()
                    .filter(|instance| node.kinds.iter().any(|kind| kind.id == instance.kind))
                    .map(instance_tree_node)
                    .collect();
                node.instance_count = rows.len();
                // Module rows are files: their only children are instances.
                node.children = rows;
                continue;
            }
            walk(&mut node.children, kinds, instances);
        }
    }
    walk(roots, kinds, instances);

    let claimed = |kind_id: &str| kinds.iter().any(|kind| kind.id == kind_id && !kind.module.is_empty());
    let mut orphan_kinds: Vec<&str> = Vec::new();
    for instance in instances {
        if !claimed(&instance.kind) && !orphan_kinds.contains(&instance.kind.as_str()) {
            orphan_kinds.push(&instance.kind);
        }
    }
    let Some(loaded) = roots.iter_mut().find(|root| root.kind == "root" && root.tier == "loaded") else {
        return;
    };
    for kind_id in orphan_kinds {
        let rows: Vec<PackageTreeNode> = instances
            .iter()
            .filter(|instance| instance.kind == kind_id)
            .map(instance_tree_node)
            .collect();
        let registered = rows.iter().any(|row| row.instance.as_ref().is_some_and(|i| i.registered));
        let name = sequencer::lisp_host::kind_name_of(kind_id);
        let package = sequencer::lisp_host::kind_package_of(kind_id);
        let (label, row_kinds) = if registered {
            (
                format!("{name} (project code)"),
                vec![TreeKind { id: kind_id.to_string(), name: name.to_string(), module: String::new() }],
            )
        } else {
            (format!("{name} (package {package} missing)"), Vec::new())
        };
        loaded.children.push(PackageTreeNode {
            label,
            kind: "orphan",
            tier: "loaded",
            path: None,
            module: None,
            detail: None,
            read_only: true,
            attached: false,
            always: false,
            instance_count: rows.len(),
            children: rows,
            kinds: row_kinds,
            instance: None,
        });
    }
}

pub(crate) fn build_package_tree(
    local_root: &Path,
    catalog: &eseqlisp::package::PackageCatalog,
    installed_dir: &Path,
    scratch_source: &str,
    init_source: &str,
) -> Vec<PackageTreeNode> {
    let mut installed = Vec::new();
    let mut factory = Vec::new();
    for package in catalog.ordered() {
        let is_installed = package.root.starts_with(installed_dir);
        let tier = if is_installed { "installed" } else { "factory" };
        let node = package_node(package, tier);
        if is_installed {
            installed.push(node);
        } else {
            factory.push(node);
        }
    }
    let mut roots = vec![
        PackageTreeNode {
            label: "Local".to_string(),
            kind: "root",
            tier: "local",
            path: Some(local_root.to_path_buf()),
            module: None,
            detail: None,
            read_only: false,
            attached: false,
            always: false,
            children: scan_tree_nodes(local_root, "local", false),
            kinds: Vec::new(),
            instance_count: 0,
            instance: None,
        },
        PackageTreeNode {
            label: "Installed".to_string(),
            kind: "root",
            tier: "installed",
            path: Some(installed_dir.to_path_buf()),
            module: None,
            detail: None,
            read_only: true,
            attached: false,
            always: false,
            children: installed,
            kinds: Vec::new(),
            instance_count: 0,
            instance: None,
        },
        PackageTreeNode {
            label: "Factory".to_string(),
            kind: "root",
            tier: "factory",
            path: None,
            module: None,
            detail: None,
            read_only: true,
            attached: false,
            always: false,
            children: factory,
            kinds: Vec::new(),
            instance_count: 0,
            instance: None,
        },
    ];
    let scratch_imports = import_modules(scratch_source);
    let init_imports = import_modules(init_source);
    mark_attachments(&mut roots, &scratch_imports, &init_imports);
    let loaded = loaded_section(&roots, &scratch_imports, &init_imports);
    roots.insert(0, loaded);
    roots
}

/// The "Loaded" section: one flat row per module the project scratch or
/// init.lisp imports, in import order, so what is live is visible at the
/// top of the list and can be removed without hunting through the tiers
/// (the way the Instruments tab leads with the engines a project uses). A
/// row keeps the file's path when some tier has it, so View Source and Copy
/// to Local work from here too; an import nobody can locate still lists,
/// since removing it is the only useful action.
fn loaded_section(
    roots: &[PackageTreeNode],
    scratch_imports: &HashSet<String>,
    init_imports: &HashSet<String>,
) -> PackageTreeNode {
    fn collect<'a>(nodes: &'a [PackageTreeNode], found: &mut Vec<&'a PackageTreeNode>) {
        for node in nodes {
            if node.kind == "module" && (node.attached || node.always) {
                found.push(node);
            }
            collect(&node.children, found);
        }
    }
    let mut located = Vec::new();
    collect(roots, &mut located);
    let mut modules = scratch_imports.iter().chain(init_imports.iter()).cloned().collect::<Vec<_>>();
    modules.sort();
    modules.dedup();
    let children = modules
        .into_iter()
        .map(|module| {
            let source = located.iter().find(|node| node.module.as_deref() == Some(&module));
            let attached = scratch_imports.contains(&module);
            let always = init_imports.contains(&module);
            // The status glyph already says project (check) or always
            // (bookmark); only the both case needs words.
            let detail = if attached && always { Some("project + always".to_string()) } else { None };
            PackageTreeNode {
                label: module.clone(),
                kind: "module",
                tier: source.map(|node| node.tier).unwrap_or("loaded"),
                path: source.and_then(|node| node.path.clone()),
                module: Some(module),
                detail,
                read_only: source.is_some_and(|node| node.read_only),
                attached,
                always,
                children: Vec::new(),
                kinds: Vec::new(),
                instance_count: 0,
                instance: None,
            }
        })
        .collect();
    PackageTreeNode {
        label: "Loaded".to_string(),
        kind: "root",
        tier: "loaded",
        path: None,
        module: None,
        detail: None,
        read_only: true,
        attached: false,
        always: false,
        children,
        kinds: Vec::new(),
        instance_count: 0,
        instance: None,
    }
}

fn mark_attachments(
    nodes: &mut [PackageTreeNode],
    scratch_imports: &HashSet<String>,
    init_imports: &HashSet<String>,
) {
    for node in nodes {
        if let Some(module) = &node.module {
            node.attached = scratch_imports.contains(module);
            node.always = init_imports.contains(module);
        }
        mark_attachments(&mut node.children, scratch_imports, init_imports);
    }
}

fn package_node(package: &eseqlisp::package::InstalledPackage, tier: &'static str) -> PackageTreeNode {
    let children = package
        .source_root
        .as_deref()
        .map(|source_root| scan_tree_nodes(source_root, tier, true))
        .unwrap_or_default();
    PackageTreeNode {
        label: package.manifest.name.clone(),
        kind: "package",
        tier,
        path: Some(package.root.clone()),
        module: package.manifest.entry.clone(),
        detail: Some(package.manifest.version.clone()),
        read_only: true,
        attached: false,
        always: false,
        children,
        kinds: Vec::new(),
        instance_count: 0,
        instance: None,
    }
}

/// Recursively list source files. Only declared modules are importable;
/// headerless helper scripts remain available for viewing their source.
fn scan_tree_nodes(
    directory: &Path,
    tier: &'static str,
    read_only: bool,
) -> Vec<PackageTreeNode> {
    let Ok(entries) = scan_directory(directory) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .map(|entry| {
            if entry.directory {
                PackageTreeNode {
                    children: scan_tree_nodes(&entry.path, tier, read_only),
                    label: entry.name,
                    kind: "folder",
                    tier,
                    path: Some(entry.path),
                    module: None,
                    detail: None,
                    read_only,
                    attached: false,
                    always: false,
                    kinds: Vec::new(),
                    instance_count: 0,
                    instance: None,
                }
            } else {
                let module = entry.module;
                let kind = if module.is_some() { "module" } else { "file" };
                // No detail column: the file name already says which module
                // it is, and a second dotted name only crowds the row (the
                // status line spells the module out on select).
                PackageTreeNode {
                    label: entry.name,
                    kind,
                    tier,
                    path: Some(entry.path),
                    detail: None,
                    module,
                    read_only,
                    attached: false,
                    always: false,
                    children: Vec::new(),
                    kinds: Vec::new(),
                    instance_count: 0,
                    instance: None,
                }
            }
        })
        .collect()
}

pub(crate) fn filter_package_tree(items: &[PackageTreeNode], query: &str) -> Vec<PackageTreeNode> {
    if query.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter_map(|item| {
            let matches = item.label.to_lowercase().contains(query)
                || item
                    .module
                    .as_ref()
                    .is_some_and(|module| module.to_lowercase().contains(query));
            // A matching module keeps every instance row: they are its
            // contents, not separate search hits.
            let children = if matches && item.kind == "module" {
                item.children.clone()
            } else {
                filter_package_tree(&item.children, query)
            };
            if item.kind == "root" || matches || !children.is_empty() {
                let mut filtered = item.clone();
                filtered.children = children;
                Some(filtered)
            } else {
                None
            }
        })
        .collect()
}

/// The Lisp tree: each root becomes a non-interactive section header
/// ("Local", "Installed", "Factory") followed by its entries at depth 0, the
/// way the instrument browser sections its list. A header never collapses,
/// so the three tiers are always in view; folders and packages below them
/// expand on click.
pub(crate) fn package_tree_to_value(roots: &[PackageTreeNode]) -> Value {
    let mut items = Vec::new();
    for root in roots {
        items.push(Rc::new(RefCell::new(crate::values::map_value(vec![
            ("label", Value::String(root.label.clone())),
            ("kind", Value::String("header".to_string())),
            ("tier", Value::String(root.tier.to_string())),
        ]))));
        let Value::List(children) = package_nodes_to_value(&root.children) else {
            continue;
        };
        items.extend(children);
    }
    Value::List(items)
}

fn package_nodes_to_value(items: &[PackageTreeNode]) -> Value {
    Value::List(
        items
            .iter()
            .map(|item| {
                let icon = match item.kind {
                    "root" | "folder" => "folder",
                    "package" | "orphan" => "project",
                    "instance" => "midi-fx",
                    _ => "document",
                };
                let mut fields: Vec<(&str, Value)> = vec![
                    ("label", Value::String(item.label.clone())),
                    ("kind", Value::String(item.kind.to_string())),
                    ("tier", Value::String(item.tier.to_string())),
                    ("icon", Value::Keyword(icon.to_string())),
                    ("read-only?", Value::Bool(item.read_only)),
                    ("attached?", Value::Bool(item.attached)),
                    ("always?", Value::Bool(item.always)),
                    ("draggable", Value::Bool(false)),
                    ("drop-target", Value::Bool(false)),
                ];
                // One trailing glyph: attached to this project wins over the
                // always-load bookmark, since the project is what the user
                // is looking at.
                if item.attached {
                    fields.push(("status-icon", Value::Keyword("check".to_string())));
                } else if item.always {
                    fields.push(("status-icon", Value::Keyword("bookmark".to_string())));
                }
                if let Some(path) = &item.path {
                    fields.push(("path", Value::String(path.display().to_string())));
                }
                if let Some(module) = &item.module {
                    fields.push(("module", Value::String(module.clone())));
                }
                if let Some(detail) = &item.detail {
                    fields.push(("detail", Value::String(detail.clone())));
                }
                if !item.kinds.is_empty() || item.kind == "orphan" {
                    fields.push((
                        "kinds",
                        Value::List(
                            item.kinds
                                .iter()
                                .map(|kind| {
                                    Rc::new(RefCell::new(crate::values::map_value(vec![
                                        ("id", Value::String(kind.id.clone())),
                                        ("name", Value::String(kind.name.clone())),
                                    ])))
                                })
                                .collect(),
                        ),
                    ));
                    fields.push(("instance-count", Value::Number(item.instance_count as f64)));
                }
                // The circled count after the check; none at zero.
                if item.instance_count > 0 {
                    fields.push(("badge", Value::Number(item.instance_count as f64)));
                }
                if let Some(instance) = &item.instance {
                    // Tree identity: sibling-unique and stable across renames.
                    fields.push(("name", Value::String(format!("instance:{}", instance.id))));
                    fields.push(("instance-id", Value::Number(instance.id as f64)));
                    fields.push(("kind-id", Value::String(instance.kind.clone())));
                    fields.push(("owner", Value::String(instance.owner.clone())));
                    fields.push((
                        "owner-rack",
                        instance.owner_rack.map(|gid| Value::Number(gid as f64)).unwrap_or(Value::Nil),
                    ));
                    fields.push(("registered?", Value::Bool(instance.registered)));
                }
                if !item.children.is_empty() {
                    fields.push(("children", package_nodes_to_value(&item.children)));
                }
                Rc::new(RefCell::new(crate::values::map_value(fields)))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_test_app() -> app::App {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(
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
                bus_effect_runtime: std::sync::Arc::new(std::sync::Mutex::new(
                    std::sync::Arc::new(Vec::new()),
                )),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            std::sync::Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry =
            sequencer::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app
    }

    /// Instance-kinds spec §8.1 at the attach seam: a manifest kind the
    /// module never `def-kind`s fails the attach before the import line is
    /// written and un-registers the module's kinds; a warning-only check
    /// attaches and hands its warnings back for the status line.
    #[test]
    fn scratch_attach_refuses_a_missing_manifest_kind_before_writing_the_import() {
        sequencer::lisp_host::clear_kind_registry();
        let mut app = kind_test_app();
        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        sequencer::lisp_host::register_graph_authoring_natives(
            &mut runtime,
            std::sync::Arc::clone(&app.state),
        );
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
        let root = temp_root("kind-attach");
        std::fs::create_dir_all(root.join("tk")).unwrap();
        std::fs::write(
            root.join("tk/kinds.lisp"),
            "(module tk.kinds)\n(def-kind real :state ((sel -1)))\n",
        )
        .unwrap();
        editor.runtime_mut().set_scoped_module_load_path(vec![eseqlisp::ModuleLoadRoot {
            path: root.clone(),
            module_prefix: None,
        }]);
        let scratch = |editor: &Editor| {
            editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
                .expect("default scratch buffer")
                .text()
        };
        let draft_before = scratch(&editor);
        let evaluated_before = app.state.scratch_source();
        let manifest: eseqlisp::package::PackageManifest =
            serde_json::from_value(serde_json::json!({
                "name": "alec/tk",
                "version": "1",
                "kinds": [{"name": "missing", "module": "tk.kinds"}],
            }))
            .unwrap();

        let error = attach_module_to_scratch(&mut editor, &mut app, "tk.kinds", |module| {
            sequencer::lisp_host::check_manifest_kinds(&manifest, module)
        })
        .expect_err("a declared kind the module never def-kinds fails the attach");
        assert!(error.contains("'missing'"), "{error}");
        assert_eq!(scratch(&editor), draft_before, "the draft gains no import line");
        assert_eq!(app.state.scratch_source(), evaluated_before);
        assert!(
            sequencer::lisp_host::kinds_defined_in_module("tk.kinds").is_empty(),
            "a refused attach forgets the module's kinds"
        );
        assert!(
            app.create_instance_recorded(
                "tk.kinds:real",
                sequencer::project::ProjectInstanceOwner::Project,
                None
            )
            .is_err(),
            "instance-create cannot instantiate a kind whose attach was refused"
        );

        // Warnings only: the import is written and the warnings come back.
        let (already, warnings) =
            attach_module_to_scratch(&mut editor, &mut app, "tk.kinds", |_| {
                Ok(vec!["def-kind 'real' is missing from the manifest".to_string()])
            })
            .expect("warnings do not fail the attach");
        assert!(!already);
        assert_eq!(warnings.len(), 1);
        assert!(scratch(&editor).contains("(import tk.kinds)"));
        assert_eq!(
            with_warnings("Attached tk.kinds".to_string(), &warnings),
            "Attached tk.kinds (warning: def-kind 'real' is missing from the manifest)"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Spec §8.3 at the host seam: `packages-detach` of a module with
    /// instances first asks, the confirm's Continue re-sends it with
    /// `:confirmed true`, and that rerun deletes the instances and drops the
    /// import from BOTH scratch texts and switches the overrides off, as one
    /// edit whose undo/redo (plus the replay hook) brings all of it back and
    /// takes it away again. Also: an import only the evaluated scratch still
    /// has (draft already edited) still switches the overrides off.
    #[test]
    fn detach_with_instances_confirms_then_detaches_and_undo_restores_everything() {
        sequencer::lisp_host::clear_kind_registry();
        let mut app = kind_test_app();
        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        sequencer::lisp_host::register_graph_authoring_natives(
            &mut runtime,
            std::sync::Arc::clone(&app.state),
        );
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
        let root = temp_root("detach-instances");
        for dir in ["t", "tk"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(
            root.join("t/factory.lisp"),
            "(module t.factory)\n(export seam)\n(def seam () \"factory\")\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tk/kinds.lisp"),
            "(module tk.kinds)\n(import t.factory)\n(def-kind real :state ((sel -1)))\n(override t.factory/seam () \"package\")\n",
        )
        .unwrap();
        // The real confirm modal's state machine, without its view.
        std::fs::write(
            // `eseq.*` modules resolve at the load root itself.
            root.join("file-dialogs.lisp"),
            "(module eseq.file-dialogs)\n(export open-confirm accept-confirm confirm-message confirm-open?)\n\
             (defstate confirm-open? false)\n(defstate confirm-message \"\")\n(defstate confirm-action nil)\n\
             (def open-confirm (message action) (set! confirm-message message) (set! confirm-action action) (set! confirm-open? true))\n\
             (def accept-confirm () (let ((action confirm-action)) (set! confirm-open? false) (if action (action) nil)))\n",
        )
        .unwrap();
        editor.runtime_mut().set_scoped_module_load_path(vec![eseqlisp::ModuleLoadRoot {
            path: root.clone(),
            module_prefix: None,
        }]);
        editor.runtime_mut().eval_str("(import eseq.file-dialogs)").unwrap();
        let draft = |editor: &Editor| {
            editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
                .expect("default scratch buffer")
                .text()
        };
        let seam = |editor: &mut Editor| editor.runtime_mut().eval_str("(t.factory/seam)").unwrap();
        let payload = |confirmed: bool| {
            crate::values::map_value(vec![
                ("module", Value::String("tk.kinds".into())),
                ("confirmed", Value::Bool(confirmed)),
            ])
        };

        attach_module_to_scratch(&mut editor, &mut app, "tk.kinds", |_| Ok(Vec::new()))
            .expect("attach");
        let kind = sequencer::lisp_host::kinds_defined_in_module("tk.kinds")
            .pop()
            .expect("the module registered its kind")
            .id;
        let owner = sequencer::project::ProjectInstanceOwner::Project;
        let a = app.create_instance_recorded(&kind, owner, None).unwrap();
        let b = app.create_instance_recorded(&kind, owner, None).unwrap();
        assert_eq!(module_instance_ids(&app, "tk.kinds"), vec![a, b]);
        assert_eq!(seam(&mut editor), Some(Value::String("package".into())));
        editor.drain_host_commands();

        // First request: only the confirm opens; nothing is detached.
        assert!(!detach_with_instances_command(&payload(false), &mut app, &mut editor));
        let confirm = editor.runtime_mut().eval_str("eseq.file-dialogs/confirm-message").unwrap();
        assert_eq!(
            confirm,
            Some(Value::String(detach_confirm_message("tk.kinds", 2).into())),
            "{confirm:?}"
        );
        assert_eq!(module_instance_ids(&app, "tk.kinds").len(), 2);
        assert!(draft(&editor).contains("(import tk.kinds)"));

        // Continue re-sends the command confirmed.
        editor.runtime_mut().eval_str("(eseq.file-dialogs/accept-confirm)").unwrap();
        let resent: Vec<Value> = editor
            .drain_host_commands()
            .into_iter()
            .filter_map(|command| match command {
                eseqlisp::host::HostCommand::Custom { name, payload } if name == "packages-detach" => {
                    Some(payload)
                }
                _ => None,
            })
            .collect();
        assert_eq!(resent.len(), 1, "Continue sends packages-detach once");
        assert!(extract_bool_from_payload(&resent[0], "confirmed"));
        assert!(detach_with_instances_command(&resent[0], &mut app, &mut editor));
        assert!(app.instances.list.is_empty(), "its instances are deleted");
        assert!(!draft(&editor).contains("(import tk.kinds)"));
        assert!(!import_modules(&app.state.scratch_source()).contains("tk.kinds"));
        assert_eq!(seam(&mut editor), Some(Value::String("factory".into())), "overrides off");

        // Undo, then the event loop's replay hook: instances, both scratch
        // lines and the overrides are back. Redo takes them away again.
        let replay = |app: &mut app::App, editor: &mut Editor, undo: bool| {
            let imports = if undo { app.history.next_undo_patch() } else { app.history.next_redo_patch() }
                .expect("a history entry")
                .replayed_scratch_imports(undo);
            let replayed = if undo { app::edit::undo(app) } else { app::edit::redo(app) };
            assert!(matches!(replayed, app::history::HistoryReplay::Applied(_)));
            let state = std::sync::Arc::clone(&app.state);
            apply_replayed_scratch_imports(editor, &state, &imports);
        };
        replay(&mut app, &mut editor, true);
        assert_eq!(module_instance_ids(&app, "tk.kinds"), vec![a, b]);
        assert!(draft(&editor).contains("(import tk.kinds)"));
        assert!(import_modules(&app.state.scratch_source()).contains("tk.kinds"));
        assert_eq!(seam(&mut editor), Some(Value::String("package".into())));
        replay(&mut app, &mut editor, false);
        assert!(app.instances.list.is_empty());
        assert!(!draft(&editor).contains("(import tk.kinds)"));
        assert!(!import_modules(&app.state.scratch_source()).contains("tk.kinds"));
        assert_eq!(seam(&mut editor), Some(Value::String("factory".into())));

        // The import lives only in the evaluated scratch (the draft line was
        // deleted without evaluating): detach still switches overrides off.
        replay(&mut app, &mut editor, true);
        let without = source_without_import(&draft(&editor), "tk.kinds").0;
        editor
            .buffers
            .iter_mut()
            .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .unwrap()
            .set_text(&without);
        assert_eq!(seam(&mut editor), Some(Value::String("package".into())));
        assert!(detach_with_instances_command(&payload(true), &mut app, &mut editor));
        assert!(app.instances.list.is_empty());
        assert!(!import_modules(&app.state.scratch_source()).contains("tk.kinds"));
        assert_eq!(seam(&mut editor), Some(Value::String("factory".into())));
        std::fs::remove_dir_all(root).unwrap();
    }

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


    // ── eseq-mods.18.1: path-keyed helpers shared by the tab and the text view ──

    #[test]
    fn detach_preserves_neighboring_forms_strings_comments_and_quoted_imports() {
        let source = concat!(
            "; (import my.euclid) is an example, not a dependency\n",
            "(def example \"Unicode λ\n(import my.euclid)\n\")\n",
            "'(import my.euclid)\n",
            "(def before 1) (import my.euclid) (def after 2) ; keep this\n",
            "(import\n  my.euclid\n  :as euclid)\n",
            "(import my.other)\n",
        );
        let (updated, removed) = source_without_import(source, "my.euclid");
        assert!(removed);
        assert!(updated.contains("(def before 1)  (def after 2) ; keep this"));
        assert!(updated.contains("(def example \"Unicode λ\n(import my.euclid)\n\")"));
        assert!(updated.contains("'(import my.euclid)"));
        assert!(updated.contains("; (import my.euclid) is an example"));
        assert_eq!(import_modules(&updated), HashSet::from(["my.other".to_string()]));
        assert_eq!(source_without_import(&updated, "my.euclid"), (updated, false));
    }

    #[test]
    fn detach_removes_only_the_canonical_import_line_and_is_idempotent() {
        let source = "(def tempo 120)\n\n(import my.euclid)\n\n(import my.other)\n";
        let (updated, removed) = source_without_import(source, "my.euclid");
        assert!(removed);
        assert_eq!(updated, "(def tempo 120)\n\n\n(import my.other)\n");
        assert!(!import_modules(&updated).contains("my.euclid"));
        assert!(import_modules(&updated).contains("my.other"));
        let (again, removed_again) = source_without_import(&updated, "my.euclid");
        assert!(!removed_again);
        assert_eq!(again, updated);
        // A half-typed unrelated form does not stop the one-line import
        // from being found, mirroring how attach derives its markers.
        let (updated, removed) = source_without_import("(import my.euclid)\n(half", "my.euclid");
        assert!(removed);
        assert_eq!(updated, "(half");
    }

    #[test]
    fn scratch_attach_then_detach_round_trips_the_draft_buffer() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
        editor
            .buffers
            .iter_mut()
            .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .expect("default scratch buffer")
            .set_text("(def project-value 1)\n");
        assert!(!attach_to_scratch(&mut editor, "my.euclid").unwrap());
        assert!(detach_from_scratch(&mut editor, "my.euclid").unwrap());
        assert!(
            !detach_from_scratch(&mut editor, "my.euclid").unwrap(),
            "a second detach finds nothing to remove"
        );
        let source = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .unwrap()
            .text();
        assert_eq!(source, "(def project-value 1)\n\n");
    }

    #[test]
    fn toggling_module_overrides_reverts_the_factory_seam_and_reattach_restores_it() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
        let root = temp_root("override-toggle");
        std::fs::create_dir_all(root.join("t")).unwrap();
        std::fs::write(
            root.join("t/factory.lisp"),
            "(module t.factory)\n(export seam)\n(def seam () \"factory\")\n",
        )
        .unwrap();
        std::fs::write(
            root.join("t/pkg.lisp"),
            "(module t.pkg)\n(import t.factory)\n(override t.factory/seam () \"package\")\n",
        )
        .unwrap();
        editor.runtime_mut().set_scoped_module_load_path(vec![eseqlisp::ModuleLoadRoot {
            path: root.clone(),
            module_prefix: None,
        }]);
        editor.runtime_mut().eval_str("(import t.pkg)").unwrap();
        let seam = |editor: &mut Editor| editor.runtime_mut().eval_str("(t.factory/seam)").unwrap();
        assert_eq!(seam(&mut editor), Some(Value::String("package".into())));
        set_module_overrides_enabled(&mut editor, "t.pkg", false);
        assert_eq!(seam(&mut editor), Some(Value::String("factory".into())));
        assert!(matches!(
            editor.runtime_mut().eval_str("(disabled-override-modules)").unwrap(),
            Some(Value::List(list)) if list.len() == 1
        ));
        set_module_overrides_enabled(&mut editor, "t.pkg", true);
        assert_eq!(seam(&mut editor), Some(Value::String("package".into())));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_detach_updates_evaluated_source_when_draft_already_removed_import() {
        let state = SequencerState::new(1, vec![]);
        state.set_scratch_source("(import my.euclid)\n(def saved 1)\n");
        let mut editor = Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        let draft = "(def unsaved 2)\n";
        editor.buffers.iter_mut().find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .unwrap().set_text(draft);
        assert!(module_still_attached_elsewhere(&state, "my.euclid", AttachmentDestination::UserInit),
            "an unevaluated draft edit does not detach the live project");
        assert!(detach_from_project(&mut editor, &state, "my.euclid").unwrap());
        assert_eq!(state.scratch_source(), "(def saved 1)\n");
        assert_eq!(editor.buffers.iter().find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
            .unwrap().text(), draft);
        assert!(!detach_from_project(&mut editor, &state, "my.euclid").unwrap());
    }

    #[test]
    fn user_init_edits_persist_with_an_open_buffer_without_saving_unrelated_draft() {
        let root = temp_root("open-init");
        std::fs::create_dir_all(&root).unwrap();
        let init = root.join("init.lisp");
        std::fs::write(&init, "(def saved 1)\n").unwrap();
        let mut editor = Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.open_file_buffer(init.clone()).unwrap();
        let buffer_id = editor.buffers.iter().position(|buffer| buffer.path.as_deref() == Some(&init)).unwrap();
        editor.buffers[buffer_id].set_text("(def saved 2)\n");
        assert!(!attach_to_user_init_at(&mut editor, &init, "my.euclid").unwrap());
        let persisted = std::fs::read_to_string(&init).unwrap();
        assert!(persisted.contains("(def saved 1)"));
        assert!(import_modules(&persisted).contains("my.euclid"));
        assert!(editor.buffers[buffer_id].text().contains("(def saved 2)"));
        assert!(import_modules(&editor.buffers[buffer_id].text()).contains("my.euclid"));
        assert!(editor.buffers[buffer_id].dirty);
        assert!(detach_from_user_init_at(&mut editor, &init, "my.euclid").unwrap());
        assert!(!import_modules(&std::fs::read_to_string(&init).unwrap()).contains("my.euclid"));
        assert!(!import_modules(&editor.buffers[buffer_id].text()).contains("my.euclid"));
        assert!(editor.buffers[buffer_id].text().contains("(def saved 2)"));
        assert!(editor.buffers[buffer_id].dirty);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn headerless_package_helpers_are_files_not_importable_modules() {
        let root = temp_root("headerless");
        std::fs::create_dir_all(root.join("helpers")).unwrap();
        std::fs::write(root.join("helpers/config.lisp"), "(def config 1)\n").unwrap();
        let tree = scan_tree_nodes(&root, "installed", true);
        let helper = &tree[0].children[0];
        assert_eq!(helper.kind, "file");
        assert!(helper.module.is_none());
        assert_eq!(helper.path.as_deref(), Some(root.join("helpers/config.lisp").as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_init_attach_and_detach_write_the_file_when_no_buffer_is_open() {
        let root = temp_root("init");
        let init = root.join("init.lisp");
        let mut editor = Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        assert!(!attach_to_user_init_at(&mut editor, &init, "my.euclid").unwrap());
        assert!(attach_to_user_init_at(&mut editor, &init, "my.euclid").unwrap());
        assert_eq!(
            std::fs::read_to_string(&init).unwrap().matches("(import my.euclid)").count(),
            1
        );
        assert!(detach_from_user_init_at(&mut editor, &init, "my.euclid").unwrap());
        assert_eq!(std::fs::read_to_string(&init).unwrap(), "");
        assert!(!detach_from_user_init_at(&mut editor, &init, "my.euclid").unwrap());
        assert!(
            !detach_from_user_init_at(&mut editor, &root.join("missing.lisp"), "x").unwrap(),
            "a missing init.lisp has nothing attached"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn module_at_path_reads_the_header_and_rejects_folders_and_headerless_files() {
        let root = temp_root("module-at-path");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("one.lisp"), "(module my.one)\n").unwrap();
        std::fs::write(root.join("plain.lisp"), "(def x 1)\n").unwrap();
        assert_eq!(module_at_path(&root.join("one.lisp")).unwrap(), "my.one");
        assert!(module_at_path(&root).unwrap_err().contains("folder"));
        assert!(module_at_path(&root.join("plain.lisp")).unwrap_err().contains("no (module"));
        let payload = crate::values::map_value([(
            "path",
            Value::String(root.join("one.lisp").display().to_string()),
        )]);
        assert_eq!(payload_module(&payload).unwrap(), "my.one");
        let payload = crate::values::map_value([("module", Value::String("my.two".into()))]);
        assert_eq!(payload_module(&payload).unwrap(), "my.two");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_tree_lists_local_installed_and_factory_tiers_with_attachment_marks() {
        let root = temp_root("tree");
        let local = root.join("local");
        std::fs::create_dir_all(local.join("my")).unwrap();
        std::fs::write(local.join("my/euclid.lisp"), "(module my.euclid)\n").unwrap();
        std::fs::write(local.join("notes.lisp"), "(def x 1)\n").unwrap();
        let installed = root.join("packages");
        std::fs::create_dir_all(installed.join("alec.drums/src")).unwrap();
        std::fs::write(
            installed.join("alec.drums/manifest.json"),
            r#"{"name":"alec/drums","version":"1","entry":"alec.drums.kit"}"#,
        )
        .unwrap();
        std::fs::write(
            installed.join("alec.drums/src/kit.lisp"),
            "(module alec.drums.kit)\n(export )\n",
        )
        .unwrap();
        let factory = root.join("factory");
        std::fs::create_dir_all(factory.join("demo.pack/src")).unwrap();
        std::fs::write(
            factory.join("demo.pack/manifest.json"),
            r#"{"name":"demo/pack","version":"1"}"#,
        )
        .unwrap();
        std::fs::write(
            factory.join("demo.pack/src/mixer.lisp"),
            "(module demo.pack.mixer)\n(def y 2)\n",
        )
        .unwrap();
        let (catalog, errors) = eseqlisp::package::PackageCatalog::scan_layered_reporting(&[
            installed.clone(),
            factory.clone(),
        ]);
        assert!(errors.is_empty(), "{errors:?}");

        let tree = build_package_tree(
            &local,
            &catalog,
            &installed,
            "(import my.euclid)\n",
            "(import alec.drums.kit)\n",
        );
        assert_eq!(
            tree.iter().map(|node| node.label.as_str()).collect::<Vec<_>>(),
            ["Loaded", "Local", "Installed", "Factory"]
        );
        let loaded = &tree[0].children;
        assert_eq!(
            loaded.iter().map(|node| (node.label.as_str(), node.detail.as_deref())).collect::<Vec<_>>(),
            [("alec.drums.kit", None), ("my.euclid", None)]
        );
        assert_eq!(loaded[1].path, Some(local.join("my/euclid.lisp")));
        assert!(loaded[0].read_only && !loaded[1].read_only);
        let tree = &tree[1..];
        let my = &tree[0].children[0];
        assert_eq!((my.label.as_str(), my.kind), ("my", "folder"));
        let euclid = &my.children[0];
        assert_eq!(euclid.module.as_deref(), Some("my.euclid"));
        assert!(euclid.attached && !euclid.always && !euclid.read_only);
        let notes = &tree[0].children[1];
        assert_eq!((notes.kind, notes.module.as_deref()), ("file", None));

        let drums = &tree[1].children[0];
        assert_eq!((drums.label.as_str(), drums.kind, drums.tier), ("alec/drums", "package", "installed"));
        assert_eq!(drums.module.as_deref(), Some("alec.drums.kit"));
        assert!(drums.always && !drums.attached && drums.read_only);
        let kit = &drums.children[0];
        assert_eq!(kit.module.as_deref(), Some("alec.drums.kit"));
        assert!(kit.always && kit.read_only);

        let demo = &tree[2].children[0];
        assert_eq!((demo.tier, demo.module.as_deref()), ("factory", None));
        assert_eq!(demo.children[0].module.as_deref(), Some("demo.pack.mixer"));

        let filtered = filter_package_tree(tree, "kit");
        assert_eq!(filtered.len(), 3, "roots always survive a filter");
        assert!(filtered[0].children.is_empty());
        assert_eq!(filtered[1].children[0].children.len(), 1);
        assert!(filtered[2].children.is_empty());

        let Value::List(rows) = package_tree_to_value(tree) else {
            panic!("tree value is a list");
        };
        let field = |index: usize, key: &str| -> Option<Value> {
            let Value::Map(map) = rows[index].borrow().clone() else {
                return None;
            };
            map.get(key).map(|value| value.borrow().clone())
        };
        // Headers then entries: Local, my/, Installed, alec/drums, Factory, demo/pack.
        assert_eq!(rows.len(), 7, "{rows:?}");
        assert_eq!(field(0, "kind"), Some(Value::String("header".into())));
        assert_eq!(field(0, "label"), Some(Value::String("Local".into())));
        assert_eq!(field(1, "label"), Some(Value::String("my".into())));
        assert_eq!(field(3, "kind"), Some(Value::String("header".into())));
        assert_eq!(field(4, "status-icon"), Some(Value::Keyword("bookmark".into())));
        assert_eq!(field(5, "label"), Some(Value::String("Factory".into())));

        let copied = copy_module_to_local(
            &installed.join("alec.drums/src/kit.lisp"),
            "alec.drums.kit",
            &local,
        )
        .unwrap();
        assert_eq!(copied, local.join("alec/drums/kit.lisp"));
        assert!(copy_module_to_local(&copied, "alec.drums.kit", &local)
            .unwrap_err()
            .contains("already has"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn module_rows_badge_their_instances_and_expand_to_one_row_per_instance() {
        let root = temp_root("kinds-tree");
        let local = root.join("local");
        std::fs::create_dir_all(&local).unwrap();
        let installed = root.join("packages");
        std::fs::create_dir_all(installed.join("alez.neural/src")).unwrap();
        std::fs::write(
            installed.join("alez.neural/manifest.json"),
            r#"{"name":"alez/neural","version":"1","entry":"alez.neural.variable-reset",
                "kinds":[{"name":"neural","module":"alez.neural.variable-reset"}]}"#,
        )
        .unwrap();
        std::fs::write(
            installed.join("alez.neural/src/variable-reset.lisp"),
            "(module alez.neural.variable-reset)\n",
        )
        .unwrap();
        std::fs::write(installed.join("alez.neural/src/other.lisp"), "(module alez.neural.other)\n")
            .unwrap();
        let (catalog, errors) =
            eseqlisp::package::PackageCatalog::scan_layered_reporting(&[installed.clone()]);
        assert!(errors.is_empty(), "{errors:?}");

        let instance = |id: u64, kind: &str, label: &str, owner: &str, rack: Option<u64>, registered: bool| {
            TreeInstance {
                id,
                kind: kind.to_string(),
                label: label.to_string(),
                owner: owner.to_string(),
                owner_rack: rack,
                registered,
            }
        };
        let instances = vec![
            instance(1, "alez/neural:neural", "neural 1", "project", None, true),
            instance(2, "alez/neural:neural", "neural 2", "Kit A", Some(9), true),
            instance(3, "gone/pkg:thing", "thing 1", "project", None, false),
        ];
        // `SEQ.instances` round-trips into tree instances.
        let value = Value::List(
            instances
                .iter()
                .map(|instance| {
                    Rc::new(RefCell::new(crate::values::map_value(vec![
                        ("id", Value::Number(instance.id as f64)),
                        ("kind", Value::String(instance.kind.clone())),
                        ("label", Value::String(instance.label.clone())),
                        ("owner-label", Value::String(instance.owner.clone())),
                        (
                            "owner-rack",
                            instance.owner_rack.map(|gid| Value::Number(gid as f64)).unwrap_or(Value::Nil),
                        ),
                        ("registered?", Value::Bool(instance.registered)),
                    ])))
                })
                .collect(),
        );
        assert_eq!(tree_instances_from_value(&value), instances);

        let mut tree =
            build_package_tree(&local, &catalog, &installed, "(import alez.neural.variable-reset)\n", "");
        let kinds = module_kinds(&catalog, &[]);
        assert_eq!(
            kinds,
            vec![TreeKind {
                id: "alez/neural:neural".into(),
                name: "neural".into(),
                module: "alez.neural.variable-reset".into(),
            }]
        );
        annotate_package_kinds(&mut tree, &kinds, &instances);

        // Loaded: the attached module row, then the missing package's
        // placeholder group.
        let loaded = &tree[0].children;
        assert_eq!(loaded.len(), 2, "{loaded:#?}");
        let module = &loaded[0];
        assert_eq!(module.kinds.len(), 1);
        assert_eq!(module.instance_count, 2);
        assert_eq!(
            module
                .children
                .iter()
                .map(|row| (row.kind, row.label.as_str(), row.detail.as_deref()))
                .collect::<Vec<_>>(),
            [("instance", "neural 1", Some("project")), ("instance", "neural 2", Some("Kit A"))]
        );
        let missing = &loaded[1];
        assert_eq!((missing.kind, missing.label.as_str()), ("orphan", "thing (package gone/pkg missing)"));
        assert_eq!(missing.instance_count, 1);
        assert_eq!(missing.children[0].detail.as_deref(), Some("project · not loaded"));
        assert!(missing.kinds.is_empty(), "a missing package offers no New");

        // The Installed tier's module row carries the same badge; a sibling
        // module without kinds gets none.
        let package = &tree[2].children[0];
        let row = |label: &str| {
            package.children.iter().find(|row| row.label == label).expect(label).clone()
        };
        assert_eq!(row("variable-reset.lisp").instance_count, 2);
        assert_eq!(row("other.lisp").instance_count, 0);
        assert!(row("other.lisp").kinds.is_empty());

        // A search hit on the module keeps all its instance rows.
        let filtered = filter_package_tree(&tree, "variable");
        assert_eq!(filtered[2].children[0].children[0].children.len(), 2);

        let Value::List(rows) = package_tree_to_value(&tree) else {
            panic!("tree value is a list");
        };
        let get = |item: &Value, key: &str| -> Option<Value> {
            let Value::Map(map) = item else { return None };
            map.get(key).map(|value| value.borrow().clone())
        };
        let module_value = rows[1].borrow().clone();
        assert_eq!(get(&module_value, "badge"), Some(Value::Number(2.0)));
        assert_eq!(get(&module_value, "instance-count"), Some(Value::Number(2.0)));
        assert_eq!(get(&module_value, "status-icon"), Some(Value::Keyword("check".into())));
        let Some(Value::List(children)) = get(&module_value, "children") else {
            panic!("the module row expands to its instances");
        };
        let second = children[1].borrow().clone();
        assert_eq!(get(&second, "kind"), Some(Value::String("instance".into())));
        assert_eq!(get(&second, "instance-id"), Some(Value::Number(2.0)));
        assert_eq!(get(&second, "owner-rack"), Some(Value::Number(9.0)));
        assert_eq!(get(&second, "name"), Some(Value::String("instance:2".into())));
        // No badge at zero.
        annotate_package_kinds(&mut tree, &kinds, &[]);
        let Value::List(rows) = package_tree_to_value(&tree) else { panic!() };
        let module_value = rows[1].borrow().clone();
        assert_eq!(get(&module_value, "badge"), None);
        assert_eq!(get(&module_value, "instance-count"), Some(Value::Number(0.0)));

        assert_eq!(detach_confirm_message("alez/neural", 3), "Detach alez/neural and delete its 3 instances?");
        assert_eq!(detach_confirm_message("alez/neural", 1), "Detach alez/neural and delete its instance?");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn editor_with_step_tabs() -> Editor {
        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
        let ui_dir = sequencer::app_paths::app_paths().ui_dir();
        for file in ["seq-core-state.lisp", "seq-step-tabs.lisp"] {
            let source = std::fs::read_to_string(ui_dir.join(file)).expect(file);
            editor.runtime_mut().eval_str(&source).unwrap_or_else(|error| {
                panic!("load {file}: {error:?}");
            });
        }
        // The layout hub is an event-time dependency of tab selection.
        editor.runtime_mut().register_native(
            "eseq.seq-layout/refresh-current-layout",
            |_args, _ctx| Ok(Value::Nil),
        );
        editor
    }

    fn registered_tabs(editor: &mut Editor) -> String {
        match editor
            .runtime_mut()
            .eval_str("(str eseq.seq-step-tabs/seq-registered-step-tabs)")
        {
            Ok(Some(Value::String(text))) => text,
            other => panic!("registered tabs: {other:?}"),
        }
    }

    #[test]
    fn source_tab_opens_as_a_closable_view_and_close_only_drops_the_tab() {
        let root = temp_root("source-tab");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("euclid.lisp");
        std::fs::write(&path, "(module my.euclid)\n").unwrap();
        let mut editor = editor_with_step_tabs();

        let buffer = open_source_tab(&mut editor, &path, "euclid.lisp", true).unwrap();
        assert!(editor.buffers.iter().any(|candidate| candidate.name == buffer));
        assert!(
            editor.buffers.iter().find(|candidate| candidate.name == buffer).unwrap().read_only,
            "installed and factory sources open read-only"
        );
        let tabs = registered_tabs(&mut editor);
        assert!(tabs.contains(&buffer) && tabs.contains(":source"), "{tabs}");
        assert!(
            matches!(
                editor.runtime_mut().eval_str(&format!(
                    "(eseq.seq-step-tabs/seq-script-step-tab? (nth eseq.seq-step-tabs/seq-registered-step-tabs 0))"
                )),
                Ok(Some(Value::Bool(false)))
            ),
            "a source tab is never mistaken for a script sequencer tab"
        );
        let rendered = editor
            .runtime_mut()
            .eval_str("(str (eseq.seq-step-tabs/seq-main-step-tabs))")
            .unwrap();
        assert!(
            matches!(&rendered, Some(Value::String(text)) if text.contains("on-close")),
            "the tab renders with a close handler: {rendered:?}"
        );

        close_source_tab(&mut editor, &buffer);
        assert!(!registered_tabs(&mut editor).contains(&buffer));
        assert!(
            !editor.buffers.iter().any(|candidate| candidate.name == buffer),
            "a clean buffer is dropped with its tab"
        );
        assert!(
            editor.drain_host_commands().is_empty(),
            "closing a source tab never queues a script-sequencer teardown"
        );

        // Unsaved edits survive the tab closing.
        let buffer = open_source_tab(&mut editor, &path, "euclid.lisp", false).unwrap();
        editor
            .buffers
            .iter_mut()
            .find(|candidate| candidate.name == buffer)
            .unwrap()
            .set_text("(module my.euclid)\n(def edited 1)\n");
        editor
            .buffers
            .iter_mut()
            .find(|candidate| candidate.name == buffer)
            .unwrap()
            .dirty = true;
        close_source_tab(&mut editor, &buffer);
        assert!(!registered_tabs(&mut editor).contains(&buffer));
        assert!(editor.buffers.iter().any(|candidate| candidate.name == buffer));

        // The project scratch registers as a tab but is never dropped.
        register_source_tab(&mut editor, "scratch", PROJECT_SCRATCH_BUFFER_NAME).unwrap();
        assert!(registered_tabs(&mut editor).contains(PROJECT_SCRATCH_BUFFER_NAME));
        close_source_tab(&mut editor, PROJECT_SCRATCH_BUFFER_NAME);
        assert!(!registered_tabs(&mut editor).contains(PROJECT_SCRATCH_BUFFER_NAME));
        assert!(editor.buffers.iter().any(|candidate| candidate.name == PROJECT_SCRATCH_BUFFER_NAME));
        std::fs::remove_dir_all(root).unwrap();
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
