//! The saved-project preparation boundary. Construction completes loading,
//! analysis, song preflight and compensation before any output file is created.
use super::*;
use super::worker::{check_cancel_file, ExportOptions};
use crate::app::{import_forms, App, LatencyPlan};
use crate::audio::{engine, offline::OfflineAudioSession};
use crate::lisp_host::{kind_is_registered, scheduler_scratch_runtime_with_fallbacks};
use crate::paths::user_init_path;
use crate::scheduler::build_scheduler_scratch_runtime;
use crate::sequencer::{AudibleSongRowApplied, SequencerState, SongPlaybackNotice};
use std::{collections::VecDeque, sync::Arc};

/// Graph teardown follows all App/session state, including on preparation errors.
pub(super) struct WorkerEngine(pub engine::HeadlessEngine);
impl Drop for WorkerEngine {
    fn drop(&mut self) { unsafe { self.0.destroy(); } }
}

pub(super) struct PreparedExport {
    app: App,
    engine: WorkerEngine,
    plan: BouncePlan,
    latency: LatencyPlan,
}

impl PreparedExport {
    pub(super) fn prepare(
        options: &ExportOptions,
        project: crate::project::ProjectFile,
        cancel: &BounceCancellation,
    ) -> io::Result<Self> {
        let name = project.name.clone();
        let owner = WorkerEngine(
            engine::init_headless_engine(options.sample_rate, 2)
                .map_err(|error| io::Error::other(error.to_string()))?,
        );
        let engine = &owner.0;
        let mut app = App::new(
            Arc::clone(&engine.state),
            engine.lg_ptr,
            engine.sample_rate,
            engine.buses.clone(),
            Arc::clone(&engine.master_recorder),
            engine.keyboard_tx.clone(),
        );
        app.sample_analysis = crate::analysis::AnalysisService::synchronous();
        app.queue_bounce_project(&name, project)
            .map_err(io::Error::other)?;
        while app.has_pending_project_load() {
            check_cancel_file(options, cancel)?;
            app.advance_pending_project_load()
                .map_err(io::Error::other)?;
            if !unsafe { crate::audiograph::prepare_graph_for_render(engine.lg_ptr.0) } {
                return Err(io::Error::other("Project graph preparation failed"));
            }
        }
        let end_beat = engine
            .state
            .committed_arrangement()
            .ok_or_else(|| invalid("Project has no arrangement"))?
            .end_beat;
        app.sample_analysis.require_complete().map_err(io::Error::other)?;
        app.publish_all_sampler_analysis_runtime();
        publish_instance_sequencers(&app, &engine.state)?;
        app.start_bounce_playback(options.selection.map(|(_, end)| end).unwrap_or(end_beat))
            .map_err(io::Error::other)?;
        let latency = unsafe { app.prepare_bounce_latency() }.map_err(io::Error::other)?;
        let plan = BouncePlan::new(
            engine.sample_rate,
            engine.block_size,
            engine.state.latest_scheduler_snapshot().transport.bpm,
            end_beat,
            options.selection,
            latency.mix_latency,
            options.tail_seconds,
        )?;
        check_cancel_file(options, cancel)?;
        Ok(Self { app, engine: owner, plan, latency })
    }

    pub(super) fn render(
        mut self, options: &ExportOptions, cancel: &BounceCancellation,
        mut progress: impl FnMut(BounceProgress),
    ) -> Result<BounceSummary, ExportError> {
        let app = &mut self.app;
        let engine = &self.engine.0;
        let plan = &self.plan;
        let latency = &self.latency;
        let source_end = plan.source_range().end;
        let mut renderer = OfflineAudioSession::new(engine, source_end).map_err(ExportError::preparation)?;
        let mut rows = VecDeque::<AudibleSongRowApplied>::new();
        render_to_wav(
            plan,
            &options.destination,
            if options.replace {
                Publication::ReplaceConfirmed
            } else {
                Publication::CreateNew
            },
            cancel,
            |start, output| {
                check_cancel_file(options, cancel)?;
                renderer.render_block_with_controls(start, output, |frame| {
                    for notice in engine.state.drain_song_playback_notices() {
                        match notice {
                            SongPlaybackNotice::RowApplied(row) => rows.push_back(row),
                            SongPlaybackNotice::StartFailed { error } => return Err(io::Error::other(error)),
                            // The render owner releases gates at E and preserves the tail;
                            // the interactive transport-stop operation resets too much.
                            SongPlaybackNotice::Ended { .. } => {}
                        }
                    }
                    if engine.state.song_playback().take_notice_overflow() {
                        return Err(io::Error::other("Export lost a song control transition"));
                    }
                    let control_frame = frame.min(source_end - 1);
                    while rows.front().is_some_and(|row| row.effective_sample <= control_frame) {
                        let row = rows.pop_front().unwrap();
                        let outcome = app.drain_due_mixer_controls(row.effective_sample);
                        if !outcome.errors.is_empty() { return Err(io::Error::other(outcome.errors.join("; "))); }
                        app.mirror_song_row_applied(&row).map_err(io::Error::other)?;
                    }
                    let outcome = app.drain_due_mixer_controls(control_frame);
                    if !outcome.errors.is_empty() { return Err(io::Error::other(outcome.errors.join("; "))); }
                    let installed = unsafe { app.prepare_bounce_latency() }.map_err(io::Error::other)?;
                    if !installed.same_compensation(latency) {
                        return Err(io::Error::other(format!(
                            "Arrangement changes processing latency or nonzero compensation at sample {frame}; export requires fixed delay compensation",
                        )));
                    }
                    Ok(())
                })
            },
            |update| progress(update),
        )
    }
}

/// Kind instances (neural sequencers etc.) publish only once Lisp has
/// evaluated their `def-kind`. The live UI republishes them whenever the kind
/// registry moves; this worker has no UI loop, so register the kinds the
/// project imports (its scratch, then the user's init.lisp imports) and
/// publish every instance before playback starts. An instance whose kind
/// still did not register fails the export rather than silently dropping
/// its notes.
fn publish_instance_sequencers(app: &App, state: &Arc<SequencerState>) -> io::Result<()> {
    let any_unregistered = || {
        app.instances.list.iter().any(|instance| !kind_is_registered(&instance.kind))
    };
    let mut errors = Vec::new();
    if any_unregistered() {
        // The scheduler's own runtime build: library sources, then the scratch.
        let (runtime, scratch_errors) =
            build_scheduler_scratch_runtime(Arc::clone(state), &state.scratch_source(), false);
        errors.extend(scratch_errors);
        let mut runtime = runtime
            .unwrap_or_else(|| scheduler_scratch_runtime_with_fallbacks(Arc::clone(state), 0, 0));
        if any_unregistered() {
            // init.lisp also configures the UI, so load only its package imports.
            let init = user_init_path()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for (module, _) in import_forms(&init) {
                if seen.insert(module.clone()) {
                    if let Err(error) = runtime.eval(&format!("(import {module})")) {
                        errors.push(format!("init.lisp import {module}: {error}"));
                    }
                }
            }
        }
    }
    app.publish_instance_sequencers();
    let missing: Vec<String> = app.instances.list.iter()
        .filter(|instance| !kind_is_registered(&instance.kind))
        .map(|instance| format!("{} ({})", instance.label, instance.kind))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "Export cannot play sequencers whose package is not attached: {}",
        missing.join(", "),
    );
    if !errors.is_empty() {
        message.push_str(&format!(" ({})", errors.join("; ")));
    }
    Err(io::Error::other(message))
}
