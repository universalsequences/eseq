//! The saved-project preparation boundary. Construction completes loading,
//! analysis, song preflight and compensation before any output file is created.
use super::*;
use super::worker::{check_cancel_file, ExportOptions};
use crate::app::{App, LatencyPlan};
use crate::audio::{engine, offline::OfflineAudioSession};
use crate::sequencer::{AudibleSongRowApplied, SongPlaybackNotice};
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
        app.start_bounce_playback().map_err(io::Error::other)?;
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
