//! What the host presents beside its model: the browser sidebar, the
//! preset and kit listings, the sound palette, the instrument and effect
//! editor, Patch Learn, MIDI capture, the song export, Settings, the agent
//! and Promote to factory (docs/kind-bindings-spec.md §14.2i).
//!
//! This record is the source of truth; views read it through the host
//! kinds. A command or a job event edits an area through its typed mutator
//! (`present_editor`, `present_learn`, `present_export`, …); a computed
//! snapshot (the sidebar, the listings, the palette) is recorded whole by
//! its publisher. Each area moves its own generation only when its value
//! changed, and the host kinds push an area's fields only when that
//! generation moved (`host_kinds::presentation`), on the next tick: a view
//! the host opens right after an edit takes what it needs as arguments.
//!
//! The older mutators still take the runtime they once mirrored legacy
//! reactive names into (eseq-0l17.76 removed the mirror and left their call
//! sites alone); `present_promote`, added then, takes none.
//!
//! The record lives on the UI thread (a thread local, like the export job
//! it describes): every publisher and the host kinds' tick run there.

mod fixture;

pub(crate) use fixture::register as register_fixture_native;

use crate::app::sound_palette::{PaletteEntry, PaletteTarget};
use crate::*;

/// One area's value and the generation that moves whenever it changes.
#[derive(Default)]
pub(crate) struct Area<T> {
    value: T,
    generation: u64,
}

impl<T> Area<T> {
    pub(crate) fn get(&self) -> &T {
        &self.value
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

impl<T: PartialEq> Area<T> {
    fn set(&mut self, value: T) {
        if self.value != value {
            self.value = value;
            self.generation += 1;
        }
    }
}

/// Everything presented, by area.
#[derive(Default)]
pub(crate) struct Presented {
    pub(crate) sidebar: Area<Sidebar>,
    pub(crate) sound_presets: Area<Vec<PresetFile>>,
    pub(crate) kit_presets: Area<Vec<PresetFile>>,
    pub(crate) palette: Area<Option<Palette>>,
    pub(crate) editor: Area<EditorView>,
    pub(crate) editor_sidebar: Area<EditorSidebar>,
    pub(crate) learn: Area<LearnView>,
    pub(crate) retro: Area<RetroView>,
    pub(crate) export: Area<ExportView>,
    pub(crate) settings: Area<SettingsView>,
    pub(crate) agent: Area<u64>,
    pub(crate) promote: Area<PromoteView>,
}

thread_local! {
    static PRESENTED: RefCell<Presented> = RefCell::new(Presented::default());
}

/// Read the record (the typed mutators below edit it).
pub(crate) fn presented<R>(f: impl FnOnce(&Presented) -> R) -> R {
    PRESENTED.with(|presented| f(&presented.borrow()))
}

/// Edit one area: when `edit` changed it, move its generation. Returns
/// whether it changed.
fn present<T: Clone + PartialEq>(
    area: fn(&mut Presented) -> &mut Area<T>,
    edit: impl FnOnce(&mut T),
) -> bool {
    PRESENTED.with(|presented| {
        let mut presented = presented.borrow_mut();
        let area = area(&mut presented);
        let old = area.value.clone();
        edit(&mut area.value);
        if area.value == old {
            return false;
        }
        area.generation += 1;
        true
    })
}

/// Edit the record outside `present`: a snapshot recorded whole
/// (`Area::set` moves its generation), or a value seeded before the first
/// host-kinds push.
fn seed(edit: impl FnOnce(&mut Presented)) {
    PRESENTED.with(|presented| edit(&mut presented.borrow_mut()));
}

/// Edit the editor's state.
pub(crate) fn present_editor(_rt: &mut Runtime, edit: impl FnOnce(&mut EditorView)) -> bool {
    present(|p| &mut p.editor, edit)
}

/// Edit the editor's macro sidebar.
pub(crate) fn present_editor_sidebar(
    _rt: &mut Runtime,
    edit: impl FnOnce(&mut EditorSidebar),
) -> bool {
    present(|p| &mut p.editor_sidebar, edit)
}

/// Edit Patch Learn.
pub(crate) fn present_learn(_rt: &mut Runtime, edit: impl FnOnce(&mut LearnView)) -> bool {
    present(|p| &mut p.learn, edit)
}

/// Edit the MIDI capture.
pub(crate) fn present_retro(_rt: &mut Runtime, edit: impl FnOnce(&mut RetroView)) -> bool {
    present(|p| &mut p.retro, edit)
}

/// Edit the song export.
pub(crate) fn present_export(_rt: &mut Runtime, edit: impl FnOnce(&mut ExportView)) -> bool {
    present(|p| &mut p.export, edit)
}

/// Edit the settings.
pub(crate) fn present_settings(_rt: &mut Runtime, edit: impl FnOnce(&mut SettingsView)) -> bool {
    present(|p| &mut p.settings, edit)
}

/// Record the agent's generation.
pub(crate) fn present_agent(_rt: &mut Runtime, generation: u64) -> bool {
    present(|p| &mut p.agent, |agent| *agent = generation)
}

/// Edit Promote to factory.
pub(crate) fn present_promote(edit: impl FnOnce(&mut PromoteView)) -> bool {
    present(|p| &mut p.promote, edit)
}

/// Whether an instrument editor session (a draft or an existing
/// instrument) is open.
pub(crate) fn instrument_editor_open() -> bool {
    presented(|p| {
        matches!(
            p.editor.get().mode.as_str(),
            "new-instrument" | "edit-instrument"
        )
    })
}

/// What the browser sidebar shows for the current track (the `browser`
/// kind), as `sync_sidebar_browser` derives it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sidebar {
    pub(crate) track: usize,
    /// `sampler`, `instrument` or `empty`.
    pub(crate) instrument_kind: &'static str,
    pub(crate) instrument: String,
    pub(crate) instrument_label: String,
    pub(crate) preset: String,
    pub(crate) presets: Vec<String>,
    pub(crate) user_presets: Vec<String>,
    pub(crate) sample: String,
    pub(crate) engines: Vec<String>,
    /// A drum rack's slots, each with its own presets.
    pub(crate) slots: Vec<SlotPresets>,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            track: 0,
            instrument_kind: "sampler",
            instrument: String::new(),
            instrument_label: String::new(),
            preset: String::new(),
            presets: Vec::new(),
            user_presets: Vec::new(),
            sample: String::new(),
            engines: Vec::new(),
            slots: Vec::new(),
        }
    }
}

/// One drum rack slot's presets (`slot-presets`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SlotPresets {
    pub(crate) track: usize,
    pub(crate) slot: usize,
    pub(crate) instrument: String,
    pub(crate) instrument_label: String,
    pub(crate) presets: Vec<String>,
    pub(crate) user_presets: Vec<String>,
    pub(crate) preset: String,
}

/// Record what the sidebar shows (`sync_sidebar_browser`).
pub(crate) fn present_sidebar(sidebar: Sidebar) {
    seed(|p| p.sidebar.set(sidebar));
}

/// A saved Sound or kit file of the browser's Sounds and Kits tabs
/// (`preset-file`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PresetFile {
    /// `sound` or `kit`.
    pub(crate) file_type: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) name: String,
    pub(crate) path: String,
    /// A kit's pad count; 0 for a Sound.
    pub(crate) pads: usize,
    pub(crate) author: String,
    pub(crate) tags: Vec<String>,
}

/// Record the saved Sounds `record_sound_presets` listed.
pub(crate) fn present_sound_presets(files: Vec<PresetFile>) {
    seed(|p| p.sound_presets.set(files));
}

/// Like [`present_sound_presets`], for the kits.
pub(crate) fn present_kit_presets(files: Vec<PresetFile>) {
    seed(|p| p.kit_presets.set(files));
}

/// The open sound palette (the `sound-palette` kind; `None` while closed).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Palette {
    pub(crate) track: usize,
    pub(crate) target: PaletteTarget,
    pub(crate) instrument: String,
    pub(crate) entries: Vec<PaletteEntry>,
}

/// Record the palette `sync_sound_palette` published.
pub(crate) fn present_palette(palette: Option<Palette>) {
    seed(|p| p.palette.set(palette));
}

/// The instrument / effect editor's state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditorView {
    /// The open session's mode (`new-instrument`, `edit-effect`, …); empty
    /// while none is open.
    pub(crate) mode: String,
    pub(crate) surface: String,
    pub(crate) buffer: String,
    pub(crate) error: String,
    pub(crate) canceling: bool,
    pub(crate) run_mode: String,
    pub(crate) active_macro: String,
    pub(crate) active_macro_action: String,
    pub(crate) open_macro: String,
}

impl Default for EditorView {
    fn default() -> Self {
        Self {
            mode: String::new(),
            surface: String::new(),
            buffer: String::new(),
            error: String::new(),
            canceling: false,
            run_mode: instrument_run_mode_label(CustomInstrumentRunMode::Instrument).to_string(),
            active_macro: String::new(),
            active_macro_action: String::new(),
            open_macro: String::new(),
        }
    }
}

/// Present an editor session opening: `mode` on `buffer`, on `surface`, with
/// a draft instrument's `run_mode` (an effect's: none), and no error.
pub(crate) fn present_editor_open(
    rt: &mut Runtime,
    mode: &str,
    buffer: &str,
    run_mode: Option<CustomInstrumentRunMode>,
    surface: EditorSurface,
) {
    present_editor(rt, |e| {
        mode.clone_into(&mut e.mode);
        buffer.clone_into(&mut e.buffer);
        e.error.clear();
        if let Some(run_mode) = run_mode {
            instrument_run_mode_label(run_mode).clone_into(&mut e.run_mode);
        }
        editor_surface_label(surface).clone_into(&mut e.surface);
    });
}

/// Present the editor closed (its surface kept for the next open).
pub(crate) fn present_editor_closed(rt: &mut Runtime) {
    present_editor(rt, |e| {
        e.canceling = false;
        e.mode.clear();
        e.error.clear();
        e.buffer.clear();
        instrument_run_mode_label(CustomInstrumentRunMode::Instrument).clone_into(&mut e.run_mode);
    });
}

/// Show `error` in the editor and refresh it now.
pub(crate) fn editor_error(editor: &mut Editor, error: impl Into<String>) {
    let error = error.into();
    let rt = editor.runtime_mut();
    present_editor(rt, |e| e.error = error);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
}

/// The patch editor's macro sidebar and asset inspector.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EditorSidebar {
    pub(crate) patch_macros: Vec<EditorMacro>,
    pub(crate) library_macros: Vec<EditorMacro>,
    pub(crate) assets: Vec<EditorAsset>,
    pub(crate) selected_asset: Option<AssetInfo>,
}

/// A defmacro: one of the patch's own, or of the saved library.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EditorMacro {
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    /// The macros its body calls (a library macro's imports).
    pub(crate) calls: Vec<String>,
    pub(crate) outputs: Vec<String>,
    pub(crate) summary: String,
    /// A library macro the patch imports.
    pub(crate) used: bool,
}

/// A file-backed tensor asset the patch can use.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EditorAsset {
    pub(crate) reference: String,
    pub(crate) tier: String,
    pub(crate) source_path: String,
}

/// The selected file-backed tensor node's asset, with its metadata (`None`
/// when the reference resolves to no valid asset).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AssetInfo {
    pub(crate) reference: String,
    pub(crate) metadata: Option<eseqlisp::editor::AssetMetadata>,
}

/// Patch Learn: the target, the training settings, progress and the result.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LearnView {
    pub(crate) target_path: String,
    pub(crate) target_name: String,
    pub(crate) phase: String,
    pub(crate) method: String,
    pub(crate) epochs: f64,
    pub(crate) cma_generations: f64,
    pub(crate) cma_population: f64,
    pub(crate) cma_sigma: f64,
    pub(crate) cma_seed: f64,
    pub(crate) cma_forward_batch: f64,
    pub(crate) local_epochs: f64,
    pub(crate) cma_continue: f64,
    pub(crate) cma_refine_epochs: f64,
    pub(crate) cma_refine_mode: String,
    pub(crate) cma_final_epochs: f64,
    pub(crate) pitch_hz: f64,
    pub(crate) gate_frames: f64,
    pub(crate) stage: String,
    pub(crate) current_epoch: f64,
    pub(crate) total_epochs: f64,
    pub(crate) loss: f64,
    pub(crate) losses: Vec<f64>,
    pub(crate) optimization_losses: Vec<f64>,
    pub(crate) plan_params: Vec<LearnPlanParam>,
    pub(crate) epoch_params: Vec<LearnEpochParam>,
    pub(crate) improvement_pct: f64,
    pub(crate) abs_distance: f64,
    pub(crate) basin_check: String,
    pub(crate) result_deltas: Vec<LearnDelta>,
    pub(crate) seeded_wav: String,
    pub(crate) final_wav: String,
    pub(crate) applied: bool,
    pub(crate) error: String,
}

/// Patch Learn's methods (the first the default) and shortlist execution
/// modes: `eseq.kinds`'s `learn-method-options` and
/// `learn-refine-mode-options`.
pub(crate) const LEARN_METHODS: [&str; 3] = [
    "Local fit + basin check",
    "Evolutionary search only",
    "Evolutionary search + training",
];
pub(crate) const LEARN_REFINE_MODES: [&str; 3] = ["Batched", "Scalar", "Auto"];

impl Default for LearnView {
    fn default() -> Self {
        Self {
            target_path: String::new(),
            target_name: String::new(),
            phase: "pick".to_string(),
            method: LEARN_METHODS[0].to_string(),
            epochs: 300.0,
            cma_generations: 12.0,
            cma_population: 0.0,
            cma_sigma: 0.2,
            cma_seed: 1.0,
            cma_forward_batch: 0.0,
            local_epochs: 0.0,
            cma_continue: 8.0,
            cma_refine_epochs: 5.0,
            cma_refine_mode: LEARN_REFINE_MODES[0].to_string(),
            cma_final_epochs: 300.0,
            pitch_hz: 0.0,
            gate_frames: 0.0,
            stage: String::new(),
            current_epoch: 0.0,
            total_epochs: 0.0,
            loss: 0.0,
            losses: Vec::new(),
            optimization_losses: Vec::new(),
            plan_params: Vec::new(),
            epoch_params: Vec::new(),
            improvement_pct: 0.0,
            abs_distance: 0.0,
            basin_check: String::new(),
            result_deltas: Vec::new(),
            seeded_wav: String::new(),
            final_wav: String::new(),
            applied: false,
            error: String::new(),
        }
    }
}

/// A param of the learn plan: learnable, frozen or unsupported.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LearnPlanParam {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) reason: String,
}

/// A param's value at the latest epoch.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LearnEpochParam {
    pub(crate) name: String,
    pub(crate) from: f64,
    pub(crate) value: f64,
    pub(crate) change: f64,
    pub(crate) step: f64,
}

/// A param's change in the result.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LearnDelta {
    pub(crate) name: String,
    pub(crate) from: f64,
    pub(crate) to: f64,
    pub(crate) change: f64,
}

impl LearnView {
    /// The training setting `field` (a `set-learn` field: the `learn` kind's
    /// name); `None` for anything else.
    pub(crate) fn setting(&self, field: &str) -> Option<Value> {
        match field {
            "method" => Some(Value::String(self.method.clone())),
            "cma-refine-mode" => Some(Value::String(self.cma_refine_mode.clone())),
            _ => Some(Value::Number(self.number(field)?)),
        }
    }

    fn number(&self, field: &str) -> Option<f64> {
        Some(match field {
            "epochs" => self.epochs,
            "cma-generations" => self.cma_generations,
            "cma-population" => self.cma_population,
            "cma-sigma" => self.cma_sigma,
            "cma-seed" => self.cma_seed,
            "cma-forward-batch" => self.cma_forward_batch,
            "local-epochs" => self.local_epochs,
            "cma-continue" => self.cma_continue,
            "cma-refine-epochs" => self.cma_refine_epochs,
            "cma-final-epochs" => self.cma_final_epochs,
            "pitch-hz" => self.pitch_hz,
            "gate-frames" => self.gate_frames,
            _ => return None,
        })
    }

    /// Set the training setting `field` to `value`, already validated
    /// (`validate_learn_setting`).
    pub(crate) fn set_setting(&mut self, field: &str, value: Value) {
        match (field, value) {
            ("method", Value::String(method)) => self.method = method,
            ("cma-refine-mode", Value::String(mode)) => self.cma_refine_mode = mode,
            (field, Value::Number(n)) => {
                if let Some(slot) = self.number_mut(field) {
                    *slot = n;
                }
            }
            _ => {}
        }
    }

    fn number_mut(&mut self, field: &str) -> Option<&mut f64> {
        Some(match field {
            "epochs" => &mut self.epochs,
            "cma-generations" => &mut self.cma_generations,
            "cma-population" => &mut self.cma_population,
            "cma-sigma" => &mut self.cma_sigma,
            "cma-seed" => &mut self.cma_seed,
            "cma-forward-batch" => &mut self.cma_forward_batch,
            "local-epochs" => &mut self.local_epochs,
            "cma-continue" => &mut self.cma_continue,
            "cma-refine-epochs" => &mut self.cma_refine_epochs,
            "cma-final-epochs" => &mut self.cma_final_epochs,
            "pitch-hz" => &mut self.pitch_hz,
            "gate-frames" => &mut self.gate_frames,
            _ => return None,
        })
    }

    /// Clear the plan, progress and result (a new target): the pick phase,
    /// the settings and target kept.
    pub(crate) fn reset(&mut self) {
        let settings = Self {
            target_path: std::mem::take(&mut self.target_path),
            target_name: std::mem::take(&mut self.target_name),
            method: std::mem::take(&mut self.method),
            epochs: self.epochs,
            cma_generations: self.cma_generations,
            cma_population: self.cma_population,
            cma_sigma: self.cma_sigma,
            cma_seed: self.cma_seed,
            cma_forward_batch: self.cma_forward_batch,
            local_epochs: self.local_epochs,
            cma_continue: self.cma_continue,
            cma_refine_epochs: self.cma_refine_epochs,
            cma_refine_mode: std::mem::take(&mut self.cma_refine_mode),
            cma_final_epochs: self.cma_final_epochs,
            pitch_hz: self.pitch_hz,
            gate_frames: self.gate_frames,
            ..Self::default()
        };
        *self = settings;
    }

    /// Show `error` (the error phase).
    pub(crate) fn fail(&mut self, error: impl Into<String>) {
        "error".clone_into(&mut self.phase);
        self.error = error.into();
    }
}

/// Present a Patch Learn error.
pub(crate) fn present_learn_error(rt: &mut Runtime, error: impl Into<String>) {
    present_learn(rt, |l| l.fail(error));
}

/// A frozen MIDI capture (the live `playing` and `position` aside).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RetroView {
    /// One per played (track, pitch) pair.
    pub(crate) lanes: Vec<String>,
    pub(crate) items: Vec<RetroItem>,
    pub(crate) duration: f64,
    pub(crate) truncated: bool,
    pub(crate) error: String,
}

/// One captured note: its lane's position and its span in seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RetroItem {
    pub(crate) lane: usize,
    pub(crate) start: f64,
    pub(crate) end: f64,
}

/// The song export modal.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExportView {
    pub(crate) default_name: String,
    pub(crate) project: String,
    pub(crate) folder: String,
    pub(crate) end: f64,
    pub(crate) busy: bool,
    pub(crate) done: bool,
    pub(crate) message: String,
    /// -1 while no render runs.
    pub(crate) percent: f64,
    pub(crate) output_name: String,
    pub(crate) reveal_label: String,
}

impl Default for ExportView {
    fn default() -> Self {
        Self {
            default_name: String::new(),
            project: String::new(),
            folder: String::new(),
            end: 0.0,
            busy: false,
            done: false,
            message: String::new(),
            percent: -1.0,
            output_name: String::new(),
            reveal_label: String::new(),
        }
    }
}

/// Promote to factory: what the open promotion copies and skips, and what
/// its commit reported (the `factory-promote` kind).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PromoteView {
    /// What is promoted: `Sound`, `kit`, `rack preset` or `preset`.
    pub(crate) target: String,
    pub(crate) destination: String,
    /// The dependencies left out, one line each.
    pub(crate) skipped: Vec<String>,
    /// Why it cannot be promoted; empty when it can.
    pub(crate) blocking: String,
    pub(crate) error: String,
    /// The name a commit found already taken: Promote then replaces it.
    pub(crate) taken: String,
}

/// Settings: the audio workers and the MIDI inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SettingsView {
    pub(crate) workers_choice: String,
    pub(crate) workers_options: Vec<String>,
    pub(crate) workers_note: String,
    pub(crate) midi_devices: Vec<MidiDevice>,
    pub(crate) midi_error: String,
    pub(crate) midi_persistent: bool,
}

/// Seed whether MIDI device choices are saved, before the first host-kinds
/// sync pushes `settings.midi-persistent` from the record.
pub(crate) fn seed_midi_persistent(persistent: bool) {
    seed(|p| p.settings.value.midi_persistent = persistent);
}

/// A MIDI input the service reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MidiDevice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) connected: bool,
    pub(crate) status: String,
}

#[cfg(test)]
mod tests;
