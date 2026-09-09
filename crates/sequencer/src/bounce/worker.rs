//! Saved-project export entry point. Run only in a dedicated process: the C
//! graph engine is process-global and must never share the interactive engine.

use super::*;
use super::session::PreparedExport;

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

pub(super) fn check_cancel_file(options: &ExportOptions, cancel: &BounceCancellation) -> io::Result<()> {
    if let Some(path) = &options.cancel_path {
        if path.try_exists()? {
            cancel.cancel();
        }
    }
    cancel.check()
}

impl ExportOptions {
    /// The UI and CLI validate the same saved data before constructing a graph.
    pub(crate) fn load_project(&self) -> io::Result<crate::project::ProjectFile> {
        let project = crate::project::load_project_from_path(&self.project)?;
        let end = project.arrangement.as_ref()
            .ok_or_else(|| invalid("Project has no arrangement"))?.end_beat;
        BouncePlan::new(self.sample_rate, crate::audio::engine::ENGINE_BLOCK_FRAMES,
            project.bpm, end, self.selection, 0, self.tail_seconds)?;
        Ok(project)
    }
}

/// Run only in a dedicated process, before any live engine is constructed.
/// The input saved project and the on-disk libraries are authoritative.
pub fn export_project(
    options: &ExportOptions, cancel: &BounceCancellation,
    progress: impl FnMut(BounceProgress),
) -> Result<BounceSummary, ExportError> {
    check_cancel_file(options, cancel).map_err(ExportError::validation)?;
    let project = options.load_project().map_err(ExportError::validation)?;
    let session = PreparedExport::prepare(options, project, cancel)
        .map_err(ExportError::preparation)?;
    session.render(options, cancel, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::audio::engine;
    use crate::bounce::session::WorkerEngine;
    use std::sync::Arc;

    #[test]
    fn invalid_saved_project_fails_validation_without_touching_destination() {
        let folder = tempfile::tempdir().unwrap();
        let project = folder.path().join("invalid.json");
        let destination = folder.path().join("export.wav");
        std::fs::write(&project, b"not JSON").unwrap();
        std::fs::write(&destination, b"original").unwrap();
        let options = ExportOptions {
            project, destination: destination.clone(), sample_rate: 48_000,
            tail_seconds: 0.0, selection: None, replace: true, cancel_path: None,
        };
        let error = export_project(&options, &BounceCancellation::default(),
            |_| panic!("invalid input must not render")).unwrap_err();
        assert_eq!(error.stage(), ExportStage::Validation);
        assert_eq!(std::fs::read(destination).unwrap(), b"original");
    }

    #[test]
    fn missing_effect_asset_fails_export_without_replacing_destination() {
        let folder = tempfile::tempdir().unwrap();
        let sample_path = folder.path().join("source.wav");
        let mut sample = hound::WavWriter::create(&sample_path, hound::WavSpec {
            channels: 1, sample_rate: 48_000, bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }).unwrap();
        sample.write_sample(0.25_f32).unwrap();
        sample.finalize().unwrap();
        for target in ["track", "bus", "rack"] {
            let missing = format!("__missing-export-{target}-ir-{}__", std::process::id());
            let project_path = folder.path().join("project.json");
            {
                let fixture = WorkerEngine(engine::init_headless_engine(48_000, 2).unwrap());
                let engine = &fixture.0;
                let mut app = App::new(Arc::clone(&engine.state), engine.lg_ptr, engine.sample_rate,
                    engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
                app.graph_controller().add_blank_sampler_track().unwrap();
                let (slot, track_count) = match target {
                    "track" => (app.add_builtin_effect_sync(0, "Convolution Reverb").unwrap(), 1),
                    "bus" => (app.add_builtin_bus_effect_sync(0, "Convolution Reverb").unwrap(), 1),
                    "rack" => {
                        let rack = app.graph_controller().add_empty_layer_rack_track().unwrap();
                        app.graph_controller().add_sampler_slot_to_rack(rack, &sample_path).unwrap();
                        (app.add_builtin_rack_slot_effect_sync(rack, 0, "Convolution Reverb").unwrap(), 2)
                    }
                    _ => unreachable!(),
                };
                app.state.set_committed_arrangement(Some(
                    crate::sequencer::ProjectArrangement::new(track_count, 0.25),
                )).unwrap();
                let mut project = app.capture_project("missing-ir").unwrap();
                let effect = match target {
                    "track" => &mut project.patterns[0].effect_slots[0][slot - crate::effects::BUILTIN_SLOT_COUNT],
                    "bus" => &mut project.buses[0].effect_slots[slot],
                    "rack" => &mut project.patterns[0].rack_tracks[1].as_mut().unwrap().slots[0].effect_slots[slot],
                    _ => unreachable!(),
                };
                effect.ir = Some(missing.clone());
                serde_json::to_writer(File::create(&project_path).unwrap(), &project).unwrap();
            }
            let destination = folder.path().join("export.wav");
            std::fs::write(&destination, b"existing recording").unwrap();
            let options = ExportOptions {
                project: project_path, destination: destination.clone(), sample_rate: 48_000,
                tail_seconds: 0.0, selection: None, replace: true, cancel_path: None,
            };
            let error = export_project(&options, &BounceCancellation::default(), |_| {}).unwrap_err();
            assert_eq!(error.stage(), ExportStage::Preparation);
            assert!(error.to_string().contains(&missing), "{error}");
            assert_eq!(std::fs::read(destination).unwrap(), b"existing recording");
        }
    }

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
            let project = app.capture_project("export-fixture").unwrap();
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
        assert!(error.is_cancelled());
        assert_eq!(error.stage(), ExportStage::Rendering);
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
        assert_eq!(error.stage(), ExportStage::Preparation);
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
