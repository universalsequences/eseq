//! Saved-project export entry point. Run only in a dedicated process: the C
//! graph engine is process-global and must never share the interactive engine.

use super::*;
use crate::app::App;
use crate::audio::{engine, offline::OfflineAudioSession};
use crate::sequencer::{AudibleSongRowApplied, SongPlaybackNotice};
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub project: PathBuf,
    pub destination: PathBuf,
    pub sample_rate: u32,
    pub tail_seconds: f64,
    pub selection: Option<(f64, f64)>,
    pub replace: bool,
    pub cancel_path: Option<PathBuf>,
}

fn check_cancel_file(options: &ExportOptions, cancel: &BounceCancellation) -> io::Result<()> {
    if let Some(path) = &options.cancel_path {
        if path.try_exists()? {
            cancel.cancel();
        }
    }
    cancel.check()
}

/// Own graph teardown even when loading, preparation or rendering fails.
struct WorkerEngine(engine::HeadlessEngine);
impl Drop for WorkerEngine {
    fn drop(&mut self) {
        unsafe {
            self.0.destroy();
        }
    }
}

/// The calling executable must enter this before creating any live engine.
/// The input file is the authoritative project for this standalone command.
pub fn export_project(
    options: &ExportOptions,
    cancel: &BounceCancellation,
    mut progress: impl FnMut(BounceProgress),
) -> io::Result<BounceSummary> {
    cancel.check()?;
    let project = crate::project::load_project_from_path(&options.project)?;
    // Reject invalid clocks/ranges before graph construction or compilation.
    let end_beat = project
        .arrangement
        .as_ref()
        .ok_or_else(|| invalid("Project has no arrangement"))?
        .end_beat;
    BouncePlan::new(
        options.sample_rate,
        512,
        project.bpm,
        end_beat,
        options.selection,
        0,
        options.tail_seconds,
    )?;
    let name = project.name.clone();
    let engine = WorkerEngine(
        engine::init_headless_engine(options.sample_rate, 2)
            .map_err(|error| io::Error::other(error.to_string()))?,
    );
    let engine = &engine.0;
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
    let source_end = plan.source_range().end;
    let mut renderer = OfflineAudioSession::new(engine, source_end)?;
    let mut rows = VecDeque::<AudibleSongRowApplied>::new();
    render_to_wav(
        &plan,
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
                if !installed.same_compensation(&latency) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_sampler_arrangement_exports_audible_exact_length_wav() {
        let folder = tempfile::tempdir().unwrap();
        let sample_path = folder.path().join("source.wav");
        let mut sample = hound::WavWriter::create(
            &sample_path,
            hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..24_000 {
            sample.write_sample(0.25_f32).unwrap();
            sample.write_sample(-0.125_f32).unwrap();
        }
        sample.finalize().unwrap();
        let project_path = folder.path().join("song.json");
        {
            let fixture = WorkerEngine(engine::init_headless_engine(48_000, 2).unwrap());
            let engine = &fixture.0;
            let mut app = App::new(
                Arc::clone(&engine.state),
                engine.lg_ptr,
                48_000,
                engine.buses.clone(),
                Arc::clone(&engine.master_recorder),
                engine.keyboard_tx.clone(),
            );
            app.graph_controller().add_track(&sample_path).unwrap();
            app.state.pattern.patterns[0].set_step_active(0, true);
            let pattern = app.state.capture_project_scenes().scenes[0].cells[0].unwrap();
            let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 1.0);
            let clip = arrangement.allocate_clip_id().unwrap();
            arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new(
                clip,
                0.0,
                1.0,
                Some(pattern.0),
            ));
            app.state
                .set_committed_arrangement(Some(arrangement))
                .unwrap();
            let project = app.capture_bounce_project("export-fixture").unwrap();
            serde_json::to_writer(File::create(&project_path).unwrap(), &project).unwrap();
        }
        let destination = folder.path().join("export.wav");
        let options = ExportOptions {
            project: project_path,
            destination: destination.clone(),
            sample_rate: 48_000,
            tail_seconds: 0.0,
            selection: None,
            replace: false,
            cancel_path: None,
        };
        let summary = export_project(&options, &BounceCancellation::default(), |_| {}).unwrap();
        assert_eq!(summary.frames, 24_000);
        assert!(
            summary.peak > 0.01,
            "export should contain the arrangement's sampler: {summary:?}"
        );
        let wav = hound::WavReader::open(&destination).unwrap();
        assert_eq!(wav.duration(), 24_000);
        assert_eq!(wav.spec().sample_format, hound::SampleFormat::Float);
        assert_eq!(wav.spec().channels, 2);
        drop(wav);

        let selected = ExportOptions {
            destination: folder.path().join("selection.wav"),
            selection: Some((0.25, 0.75)),
            tail_seconds: 0.01,
            ..options.clone()
        };
        let selection = export_project(&selected, &BounceCancellation::default(), |_| {}).unwrap();
        assert_eq!(selection.frames, 12_000 + 480);
        assert!(selection.peak > 0.01);

        std::fs::write(&destination, b"existing destination").unwrap();
        let replace = ExportOptions {
            replace: true,
            ..options.clone()
        };
        let cancel = BounceCancellation::default();
        let error = export_project(&replace, &cancel, |_| cancel.cancel()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"existing destination"
        );

        let mut missing = crate::project::load_project_from_path(&options.project).unwrap();
        missing.patterns[0].sample_paths[0] = Some(
            folder
                .path()
                .join("missing.wav")
                .to_string_lossy()
                .into_owned(),
        );
        serde_json::to_writer(File::create(&options.project).unwrap(), &missing).unwrap();
        let error = export_project(&replace, &BounceCancellation::default(), |_| {}).unwrap_err();
        assert!(
            error.to_string().contains("cannot reopen sample"),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"existing destination"
        );
    }
}
