//! A headless-engine `App` for tests that need a real graph (rack slots,
//! scene launches, project round trips), with the scene helpers they share.

use std::path::Path;
use std::sync::Arc;

use super::App;

/// The bundled plate impulse: a short, always-present test sample.
pub(crate) const PLATE: &str = "../../content/impulses/lexicon-300-rich-plate.wav";
/// The bundled piano impulse: a second distinct test sample.
pub(crate) const PIANO: &str = "../../content/impulses/prepared/yamaha-g5.wav";

/// An `App` on a headless engine; dropping it tears the engine down after
/// the app.
pub(crate) struct Rig {
    engine: Option<crate::audio::engine::HeadlessEngine>,
    app: Option<App>,
}

impl Rig {
    pub(crate) fn new() -> Self {
        let engine = crate::audio::engine::init_headless_engine(48_000, 2).unwrap();
        let app = App::new(
            Arc::clone(&engine.state),
            engine.lg_ptr,
            engine.sample_rate,
            engine.buses.clone(),
            Arc::clone(&engine.master_recorder),
            engine.keyboard_tx.clone(),
        );
        Self {
            engine: Some(engine),
            app: Some(app),
        }
    }

    pub(crate) fn app(&mut self) -> &mut App {
        self.app.as_mut().unwrap()
    }

    /// Apply pending graph edits, as the audio callback would.
    pub(crate) fn render(&self) {
        let lg = self.engine.as_ref().unwrap().lg_ptr;
        unsafe { crate::audiograph::prepare_graph_for_render(lg.0) };
    }

    /// Load `project` (through a JSON round trip, as a saved file would),
    /// rendering until the staged load finishes.
    pub(crate) fn reload(&mut self, name: &str, project: crate::project::ProjectFile) {
        let encoded = serde_json::to_value(&project).unwrap();
        let restored: crate::project::ProjectFile = serde_json::from_value(encoded).unwrap();
        self.app().queue_loaded_project(name, restored).unwrap();
        for _ in 0..400 {
            if !self.app().has_pending_project_load() {
                break;
            }
            self.app().advance_pending_project_load().unwrap();
            self.render();
        }
        assert!(
            !self.app().has_pending_project_load(),
            "the project finished loading"
        );
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        drop(self.app.take());
        if let Some(engine) = self.engine.take() {
            unsafe { engine.destroy() };
        }
    }
}

/// Launch scene `scene` now.
pub(crate) fn launch(app: &mut App, scene: usize) {
    let names = app.tracks.clone();
    app.state
        .launch_scene(
            scene,
            names.len(),
            &app.graph.track_buffer_ids.clone(),
            &app.graph.track_sample_rates.clone(),
            &names,
            &app.graph.track_instrument_types.clone(),
        )
        .expect("scene launches");
}

/// A new scene cloned from the current one; its index.
pub(crate) fn new_scene(app: &mut App) -> usize {
    let names = app.tracks.clone();
    app.state.clone_pattern(
        names.len(),
        &app.graph.track_buffer_ids.clone(),
        &app.graph.track_sample_rates.clone(),
        &names,
        &app.graph.track_instrument_types.clone(),
    )
}

/// A plain first track, then an Instrument Rack track (its index) with a
/// sampler slot per `samples`.
pub(crate) fn rack_with_samplers(app: &mut App, samples: &[&str]) -> usize {
    app.graph_controller()
        .add_track(Path::new(PLATE))
        .expect("a first track");
    let rack = app.graph_controller().add_empty_layer_rack_track().unwrap();
    for sample in samples {
        app.graph_controller()
            .add_sampler_slot_to_rack(rack, Path::new(sample))
            .expect("a sampler slot");
    }
    rack
}
