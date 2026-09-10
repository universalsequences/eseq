use crate::*;
use sequencer::bounce::{
    job::{ExportJob, ExportJobSettings, WorkerStatus},
};
use std::cell::RefCell;

pub(super) const COMMANDS: &[&str] = &[
    "export-song-open",
    "export-song-start",
    "export-song-cancel",
    "export-song-reveal",
];
thread_local! {
    static JOB: RefCell<Option<ExportJob>> = const { RefCell::new(None) };
    static LAST_POLL: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

fn message(editor: &mut Editor, text: String) {
    editor
        .runtime_mut()
        .set_reactive("EXPORT", "export-message", Value::String(text));
    refresh(editor);
}

fn refresh(editor: &mut Editor) {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

/// Publish one complete job transition and refresh the already-open modal.
/// This must not rely on transport/UI epochs or another user gesture.
pub(crate) fn publish_job_status(editor: &mut Editor, status: &WorkerStatus, running: bool) {
    let text = match status {
        WorkerStatus::Preparing => "Preparing export…".into(),
        WorkerStatus::Rendering { percent } => format!("Exporting audio — {percent}%"),
        WorkerStatus::Completed { tail_warning, .. } => if *tail_warning {
            "Export complete. Audio remains at the end; consider a longer tail."
        } else {
            "Export complete."
        }
        .into(),
        WorkerStatus::Cancelled => "Export cancelled.".into(),
        WorkerStatus::Failed { message, .. } => format!("Export failed: {message}"),
    };
    let percent = match status {
        WorkerStatus::Rendering { percent } => *percent as f64,
        WorkerStatus::Completed { .. } => 100.0,
        _ => -1.0,
    };
    let rt = editor.runtime_mut();
    rt.set_reactive("EXPORT", "export-busy", Value::Bool(running));
    rt.set_reactive(
        "EXPORT",
        "export-done",
        Value::Bool(!running && matches!(status, WorkerStatus::Completed { .. })),
    );
    rt.set_reactive("EXPORT", "export-percent", Value::Number(percent));
    message(editor, text);
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    let result = (|| -> Result<(), String> {
        match name {
            "export-song-open" => {
                let running = JOB.with(|job| job.borrow().as_ref().is_some_and(ExportJob::running));
                if !running {
                    let name = app.current_project_name.as_deref().unwrap_or("Untitled");
                    let paths = sequencer::app_paths::app_paths();
                    let end = app.state.committed_arrangement()
                        .ok_or("Project has no arrangement")?.end_beat;
                    let folder = paths.recordings_dir();
                    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                    let filename = sequencer::bounce::job::next_name(&folder, name)
                        .map_err(|e| e.to_string())?;
                    JOB.with(|value| *value.borrow_mut() = None);
                    let rt = editor.runtime_mut();
                    for (key, value) in [
                        ("export-default-name", Value::String(filename)),
                        ("export-project", Value::String(name.to_owned())),
                        ("export-folder", Value::String(folder.display().to_string())),
                        ("export-end", Value::Number(end)),
                        ("export-busy", Value::Bool(false)),
                        ("export-done", Value::Bool(false)),
                        ("export-message", Value::String(String::new())),
                        ("export-percent", Value::Number(-1.0)),
                        (
                            "export-reveal-label",
                            Value::String(
                                if cfg!(target_os = "macos") {
                                    "Show in Finder"
                                } else {
                                    "Open folder"
                                }
                                .into(),
                            ),
                        ),
                    ] {
                        rt.set_reactive("EXPORT", key, value);
                    }
                    rt.eval_str("(eseq.export-song/reset)")
                        .map_err(|e| format!("{e:?}"))?;
                }
                if !editor.switch_active_tile_to_buffer_named("*arrangement*") {
                    editor.switch_active_tile_to_buffer_named("*sequencer*");
                }
                editor
                    .runtime_mut()
                    .eval_str("(eseq.export-song/open)")
                    .map_err(|e| format!("{e:?}"))?;
            }
            "export-song-start" => {
                if JOB.with(|job| job.borrow().as_ref().is_some_and(ExportJob::running)) {
                    return Err("An export is already running".into());
                }
                let Value::Map(map) = payload else {
                    return Err("Missing export settings".into());
                };
                let read = |key| map_string(&map, key).ok_or_else(|| format!("Missing {key}"));
                let filename = read("name")?;
                let rate: u32 = read("sample-rate")?
                    .parse()
                    .map_err(|_| "Invalid sample rate")?;
                let tail: f64 = read("tail")?.parse().map_err(|_| "Invalid tail duration")?;
                let selection = if read("range")? == "Beat range" {
                    Some((
                        read("start")?
                            .parse::<f64>()
                            .map_err(|_| "Invalid start beat")?,
                        read("end")?
                            .parse::<f64>()
                            .map_err(|_| "Invalid end beat")?,
                    ))
                } else {
                    None
                };
                pull_named_scratch_buffer_into_project(editor, app);
                let project = app.capture_export_project()?;
                let destination = sequencer::bounce::job::destination(
                    &sequencer::app_paths::app_paths().recordings_dir(),
                    &filename,
                )
                .map_err(|e| e.to_string())?;
                let job = ExportJob::start(
                    &std::env::current_exe().map_err(|e| e.to_string())?,
                    &project,
                    ExportJobSettings {
                        destination,
                        sample_rate: rate,
                        tail_seconds: tail,
                        selection,
                    },
                )
                .map_err(|e| e.to_string())?;
                editor.runtime_mut().set_reactive(
                    "EXPORT",
                    "export-output-name",
                    Value::String(
                        job.destination
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    ),
                );
                JOB.with(|value| *value.borrow_mut() = Some(job));
                publish_job_status(editor, &WorkerStatus::Preparing, true);
            }
            "export-song-cancel" => {
                JOB.with(|job| job.borrow().as_ref().map(ExportJob::cancel).transpose())
                    .map_err(|e| e.to_string())?;
                message(editor, "Cancelling…".into());
            }
            "export-song-reveal" => {
                let path = JOB
                    .with(|job| {
                        job.borrow()
                            .as_ref()
                            .filter(|j| {
                                !j.running() && matches!(j.status, WorkerStatus::Completed { .. })
                            })
                            .map(|j| j.destination.clone())
                    })
                    .ok_or("No completed export to reveal")?;
                #[cfg(target_os = "macos")]
                let mut command = {
                    let mut c = std::process::Command::new("open");
                    c.arg("-R").arg(&path);
                    c
                };
                #[cfg(not(target_os = "macos"))]
                let mut command = {
                    let mut c = std::process::Command::new("xdg-open");
                    c.arg(path.parent().unwrap());
                    c
                };
                let status = command.status().map_err(|e| e.to_string())?;
                if !status.success() {
                    return Err(format!("Could not open the recordings folder ({status})"));
                }
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        message(editor, error.clone());
        editor.show_transient_message(error);
    }
    refresh(editor);
}

pub(crate) fn poll(editor: &mut Editor) {
    let due = LAST_POLL.with(|last| {
        let mut last = last.borrow_mut();
        if last.is_some_and(|t| t.elapsed() < Duration::from_millis(100)) {
            return false;
        }
        *last = Some(Instant::now());
        true
    });
    if !due {
        return;
    }
    JOB.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(job) = slot.as_mut().filter(|job| job.running()) else {
            return;
        };
        let before = job.status.clone();
        if let Err(error) = job.poll() {
            let _ = job.cancel();
            message(editor, format!("Export status error: {error}"));
            return;
        }
        if before == job.status && job.running() {
            return;
        }
        publish_job_status(editor, &job.status, job.running());
    });
}
