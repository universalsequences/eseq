//! Promote to factory (eseq-jhmx): capture a track, a drum rack or the
//! current preset exactly as the browser's save flows do, strip everything a
//! fresh install would not have (`crate::factory_promote`), and write the
//! result into the checkout's `content/` tree.

use super::*;
use crate::factory_promote::{
    sanitize_kit, sanitize_sound, write_factory_file, FactoryIndex, FactoryKind,
    LiveFactoryIndex, PromoteReport, SoundOutcome,
};

/// What a promotion captures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromoteTarget {
    /// Any instrument track, as a Sound.
    Sound { track: usize },
    /// A drum rack, as a kit (pads and bus chain; no clips).
    Kit { group_id: u64 },
    /// The track's current preset: an instrument rack's rack preset, or a
    /// custom instrument's preset in that instrument's factory bank.
    Preset { track: usize },
}

/// What the promote modal shows before anything is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromotePreview {
    /// "Sound", "kit", "rack preset" or "preset".
    pub kind_label: String,
    pub default_name: String,
    /// Where the promotion writes, relative to the checkout.
    pub destination: String,
    /// One line per dependency that will be left out.
    pub skipped: Vec<String>,
    /// Why nothing can be promoted, if so.
    pub blocking: Option<String>,
}

/// A promotion's captured, sanitized payload.
enum Promotion {
    Sound(crate::project::ProjectSoundPreset),
    Kit(crate::project::ProjectKitPreset),
    RackPreset(crate::project::ProjectSoundPreset),
    InstrumentPreset { instrument: String, preset: crate::lisp_host::InstrumentPreset },
}

impl App {
    /// The target a promote command means right now: the cursor track, or
    /// the drum rack the cursor track sits in.
    pub fn factory_promote_target(&self, kind: &str) -> Result<PromoteTarget, String> {
        let track = self.ui.cursor_track;
        if track >= self.tracks.len() {
            return Err("Select a track first".to_string());
        }
        match kind {
            "sound" => Ok(PromoteTarget::Sound { track }),
            "preset" => Ok(PromoteTarget::Preset { track }),
            "kit" => self
                .groups
                .iter()
                .find(|group| group.rack.is_some() && group.members.contains(&track))
                .map(|group| PromoteTarget::Kit { group_id: group.id })
                .ok_or_else(|| "Select a pad of a drum rack to promote it as a kit".to_string()),
            other => Err(format!("Unknown promotion '{other}'")),
        }
    }

    /// Capture and vet `target` without writing anything.
    pub fn preview_factory_promotion(&mut self, target: PromoteTarget) -> PromotePreview {
        let default_name = self.factory_promote_default_name(target);
        let mut preview = PromotePreview {
            kind_label: String::new(),
            default_name: default_name.clone(),
            ..PromotePreview::default()
        };
        let paths = crate::app_paths::app_paths();
        if let Err(error) = crate::factory_promote::ensure_writable_factory(paths) {
            preview.blocking = Some(error);
            return preview;
        }
        match self.capture_factory_promotion(target, &default_name) {
            Ok((promotion, report)) => {
                preview.kind_label = promotion_label(&promotion).to_string();
                preview.destination = promotion_destination(paths, &promotion);
                preview.skipped = report.skipped;
            }
            Err(error) => preview.blocking = Some(error),
        }
        preview
    }

    /// Capture, vet and write `target` as `name`. Returns the file written
    /// and what was left out.
    pub fn promote_to_factory(
        &mut self,
        target: PromoteTarget,
        name: &str,
        overwrite: bool,
    ) -> Result<(PathBuf, PromoteReport), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give it a name first".to_string());
        }
        let paths = crate::app_paths::app_paths();
        crate::factory_promote::ensure_writable_factory(paths)?;
        let (promotion, report) = self.capture_factory_promotion(target, name)?;
        let path = match promotion {
            Promotion::Sound(sound) => {
                write_factory_file(paths, FactoryKind::Sound, name, &sound, overwrite)?
            }
            Promotion::Kit(kit) => write_factory_file(paths, FactoryKind::Kit, name, &kit, overwrite)?,
            Promotion::RackPreset(preset) => {
                write_factory_file(paths, FactoryKind::RackPreset, name, &preset, overwrite)?
            }
            Promotion::InstrumentPreset { instrument, preset } => {
                let exists = crate::lisp_host::load_instrument_preset_names(&instrument)
                    .unwrap_or_default()
                    .iter()
                    .any(|existing| existing == name);
                if exists && !overwrite {
                    return Err(format!("A preset named '{name}' already exists"));
                }
                crate::lisp_host::promote_instrument_preset_to_factory(&instrument, &preset)
                    .map_err(|error| error.to_string())?
            }
        };
        Ok((path, report))
    }

    fn factory_promote_default_name(&self, target: PromoteTarget) -> String {
        match target {
            PromoteTarget::Sound { track } => {
                self.tracks.get(track).cloned().unwrap_or_default()
            }
            PromoteTarget::Kit { group_id } => self
                .groups
                .iter()
                .find(|group| group.id == group_id)
                .map(|group| group.name.clone())
                .unwrap_or_default(),
            PromoteTarget::Preset { track } => self
                .state
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get(track)
                .and_then(|state| state.loaded_preset.clone())
                .or_else(|| self.tracks.get(track).cloned())
                .unwrap_or_default(),
        }
    }

    fn capture_factory_promotion(
        &mut self,
        target: PromoteTarget,
        name: &str,
    ) -> Result<(Promotion, PromoteReport), String> {
        let paths = crate::app_paths::app_paths();
        let index = LiveFactoryIndex::new(paths);
        let mut report = PromoteReport::default();
        let promotion = match target {
            PromoteTarget::Sound { track } => {
                let mut sound = self.capture_track_as_container_preset(
                    track,
                    name,
                    Vec::new(),
                    String::new(),
                )?;
                if sanitize_sound(&mut sound, &index, "", &mut report) == SoundOutcome::Empty {
                    return Err(nothing_factory(&report, "Sound"));
                }
                Promotion::Sound(sound)
            }
            PromoteTarget::Kit { group_id } => {
                let (mut kit, warnings) =
                    self.capture_rack_as_kit(group_id, name, Vec::new(), String::new(), &[])?;
                report.skipped.extend(warnings);
                sanitize_kit(&mut kit, &index, &mut report)?;
                Promotion::Kit(kit)
            }
            PromoteTarget::Preset { track } => match self.graph.track_instrument_types.get(track) {
                Some(InstrumentType::Rack) => {
                    let mut preset = self.capture_track_as_container_preset(
                        track,
                        name,
                        Vec::new(),
                        String::new(),
                    )?;
                    if sanitize_sound(&mut preset, &index, "", &mut report) == SoundOutcome::Empty {
                        return Err(nothing_factory(&report, "rack preset"));
                    }
                    Promotion::RackPreset(preset)
                }
                Some(InstrumentType::Custom) => {
                    let (instrument, preset) = self.capture_current_instrument_preset(track, name)?;
                    if !index.instrument(&instrument) {
                        return Err(format!(
                            "'{instrument}' is not a factory instrument, so its presets cannot ship"
                        ));
                    }
                    Promotion::InstrumentPreset { instrument, preset }
                }
                _ => return Err("This track has no preset to promote".to_string()),
            },
        };
        Ok((promotion, report))
    }

    /// The track's live instrument parameters as a preset named `name`, the
    /// same capture "Save preset" makes.
    fn capture_current_instrument_preset(
        &self,
        track: usize,
        name: &str,
    ) -> Result<(String, crate::lisp_host::InstrumentPreset), String> {
        let engine_id = self
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|id| *id)
            .ok_or_else(|| "The track's instrument is not loaded".to_string())?;
        let instrument = self
            .editor
            .engine_registry
            .get(engine_id)
            .map(|engine| engine.name.clone())
            .ok_or_else(|| "The track's instrument is not loaded".to_string())?;
        let descriptor = self
            .graph
            .instrument_descriptors
            .get(track)
            .cloned()
            .ok_or_else(|| "Instrument descriptor unavailable".to_string())?;
        let slot = &self.state.pattern.instrument_slots[track];
        let params = descriptor
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| (param.name.clone(), slot.defaults.get(idx)))
            .collect();
        let preset = crate::lisp_host::InstrumentPreset {
            id: name.to_string(),
            name: name.to_string(),
            base_note_offset: self.instrument_base_note_offset(track),
            params,
            key_locks: crate::effects::capture_key_locks_by_param_name(slot, &descriptor),
        };
        Ok((instrument, preset))
    }
}

fn nothing_factory(report: &PromoteReport, what: &str) -> String {
    let mut message = format!("Nothing in this {what} is factory content");
    if let Some(first) = report.skipped.first() {
        message.push_str(&format!(" ({first})"));
    }
    message
}

fn promotion_label(promotion: &Promotion) -> &'static str {
    match promotion {
        Promotion::Sound(_) => "Sound",
        Promotion::Kit(_) => "kit",
        Promotion::RackPreset(_) => "rack preset",
        Promotion::InstrumentPreset { .. } => "preset",
    }
}

fn promotion_destination(paths: &crate::app_paths::AppPaths, promotion: &Promotion) -> String {
    let dir = match promotion {
        Promotion::Sound(_) => paths.factory_sounds_dir(),
        Promotion::Kit(_) => paths.kits_dir(),
        Promotion::RackPreset(_) => paths.rack_presets_dir(),
        Promotion::InstrumentPreset { instrument, .. } => {
            return format!("the factory preset bank of '{instrument}'");
        }
    };
    let root = paths.factory_root();
    let relative = dir.strip_prefix(root.parent().unwrap_or(&root)).unwrap_or(&dir);
    format!("{}/", relative.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Previewing captures and vets without writing: a blank sampler ships
    /// whole as a Sound, and targets that do not apply say why.
    #[test]
    fn preview_vets_the_live_track_without_writing() {
        let engine = crate::audio::engine::init_headless_engine(48_000, 2).unwrap();
        let lg = engine.lg_ptr;
        let mut app = App::new(engine.state, lg, engine.sample_rate,
            engine.buses, engine.master_recorder, engine.keyboard_tx);
        assert!(app.factory_promote_target("sound").is_err(), "no track yet");
        app.graph_controller().add_blank_sampler_track().unwrap();
        app.ui.cursor_track = 0;

        let target = app.factory_promote_target("sound").unwrap();
        assert_eq!(target, PromoteTarget::Sound { track: 0 });
        let preview = app.preview_factory_promotion(target);
        assert_eq!(preview.blocking, None);
        assert_eq!(preview.kind_label, "Sound");
        assert!(preview.skipped.is_empty(), "{:?}", preview.skipped);
        assert!(preview.destination.ends_with("content/sounds/"), "{}", preview.destination);
        assert_eq!(preview.default_name, app.tracks[0]);

        assert!(app.factory_promote_target("kit").unwrap_err().contains("drum rack"));
        let preset = app.preview_factory_promotion(PromoteTarget::Preset { track: 0 });
        assert!(preset.blocking.is_some_and(|reason| reason.contains("no preset")));
        assert!(app.promote_to_factory(target, "  ", false).is_err(), "a name is required");
        drop(app);
        unsafe {
            crate::audiograph::engine_stop_workers();
            crate::audiograph::destroy_live_graph(lg.0);
        }
    }
}
