//! File › Export Instrument as Plugin… (docs/plugin-export-spec.md §9): the
//! current track's instrument, with the track's current values as the
//! plugin's defaults, exported as CLAP + VST3 into the per-user plug-in
//! folders. Compiling takes seconds, so the export runs on a worker and
//! `poll` reports it.

use crate::*;
use sequencer::plugin_export::{ExportOptions, ExportReport, export_instrument_plugin};
use std::cell::RefCell;
use std::sync::mpsc::{Receiver, TryRecvError};

pub(super) const COMMANDS: &[&str] = &["export-instrument-plugin"];

thread_local! {
    static JOB: RefCell<Option<Receiver<Result<ExportReport, String>>>> = const { RefCell::new(None) };
}

pub(super) fn handle(name: &str, _payload: Value, app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    if name != "export-instrument-plugin" {
        return;
    }
    if JOB.with(|job| job.borrow().is_some()) {
        editor.show_transient_message("A plugin export is already running");
        return;
    }
    let track = ctx.shared.current_track.load(Ordering::Relaxed);
    let options = match export_options(app, track) {
        Ok(options) => options,
        Err(error) => {
            editor.show_toast(format!("Export plugin: {error}"), eseqlisp::ToastKind::Error);
            return;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let label = options.instrument.clone();
    let spawned = std::thread::Builder::new()
        .name("plugin-export".into())
        .spawn(move || {
            let _ = tx.send(export_instrument_plugin(&options));
        });
    match spawned {
        Ok(_) => {
            JOB.with(|job| *job.borrow_mut() = Some(rx));
            editor.show_toast(format!("Exporting {label} as a plugin…"), eseqlisp::ToastKind::Loading);
        }
        Err(error) => editor.show_toast(format!("Export plugin: {error}"), eseqlisp::ToastKind::Error),
    }
    editor.mark_needs_redraw();
}

/// The track's instrument and its current values (by param name, in the
/// instrument's stored units — what the DSP cells hold).
fn export_options(app: &app::App, track: usize) -> Result<ExportOptions, String> {
    let descriptor = app
        .graph
        .instrument_descriptors
        .get(track)
        .ok_or("the current track has no instrument")?;
    let slot = app
        .state
        .pattern
        .instrument_slots
        .get(track)
        .ok_or("the current track has no instrument")?;
    if descriptor.name.is_empty() {
        return Err("the current track has no instrument".into());
    }
    let mut options = ExportOptions::new(descriptor.name.clone());
    for (index, param) in descriptor.params.iter().enumerate() {
        options.param_values.insert(param.name.clone(), slot.defaults.get(index));
    }
    Ok(options)
}

/// Report a finished export (called every reactive tick).
pub(crate) fn poll(editor: &mut Editor) {
    let finished = JOB.with(|job| {
        let mut job = job.borrow_mut();
        let result = match job.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the export worker stopped".into()),
        };
        *job = None;
        Some(result)
    });
    match finished {
        Some(Ok(report)) => {
            let formats: Vec<String> = report
                .bundles
                .iter()
                .filter_map(|b| b.extension().map(|e| e.to_string_lossy().to_uppercase()))
                .collect();
            editor.show_toast(
                format!("Exported {} ({}) — rescan plug-ins in your DAW", report.name, formats.join(" + ")),
                eseqlisp::ToastKind::Success,
            );
            editor.mark_needs_redraw();
        }
        Some(Err(error)) => {
            editor.show_toast(format!("Export plugin failed: {error}"), eseqlisp::ToastKind::Error);
            editor.mark_needs_redraw();
        }
        None => {}
    }
}
