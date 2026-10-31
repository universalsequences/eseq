//! The per-instrument payload an exported plugin bundle carries in
//! `Contents/Resources/eseq-payload/`. The exporter (`sequencer::plugin_export`)
//! resolves everything the shell needs from eseq's own manifest parser into
//! [`PluginSpec`] — params with their state cells, the DSP's input routes,
//! audio channels and cell defaults — so the shell (`eseq-plugin`) reads plain
//! numbers and never re-derives DGen manifest rules.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory, relative to the bundle's `Contents/Resources`, holding the payload.
pub const PAYLOAD_DIR: &str = "eseq-payload";
/// The payload's identity, params and DSP layout.
pub const SPEC_FILE: &str = "plugin.json";
pub const SPEC_FORMAT: u32 = 1;
/// Slots after a voice's state span the host keeps zeroed.
pub const STATE_REDZONE_SLOTS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginSpec {
    pub format: u32,
    /// CLAP id; the VST3 class id is derived from it by clap-wrapper, so it
    /// must never change across re-exports of the same instrument.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub url: String,
    /// CLAP feature strings, e.g. `["instrument", "drum", "stereo"]`.
    pub features: Vec<String>,
    pub dsp: DspSpec,
    /// Voices one plugin instance plays.
    pub polyphony: usize,
    /// Host-visible params, in display order.
    pub params: Vec<ParamSpec>,
    #[serde(default)]
    pub ui: Option<UiSpec>,
    /// The eseq instrument this was exported from (logical name).
    #[serde(default)]
    pub source_instrument: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DspSpec {
    /// Payload-relative path of the compiled DGen library.
    pub library: String,
    /// Payload-relative path of its manifest; the shell reads only its
    /// `tensorInitData` (weights baked in at compile time).
    pub manifest: String,
    /// `--voices` the library was compiled with: the number of scratch slices
    /// in its file-scope statics. Instances of one plugin share the image, so
    /// voices claim distinct slices process-wide.
    pub voice_slots: usize,
    pub layout: DspLayout,
}

/// One voice's DSP, as eseq's manifest parser resolved it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DspLayout {
    pub total_memory_slots: usize,
    /// The cell selecting the voice's scratch slice.
    pub voice_cell: Option<usize>,
    /// Most frames one process call may render.
    pub max_frames: usize,
    pub n_inputs: usize,
    pub n_outputs: usize,
    /// Output channels mixed to the stereo bus (equal for a mono instrument).
    pub audio_left: usize,
    pub audio_right: usize,
    /// The `@amp` output: nonzero while a released voice still sounds.
    pub amp_channel: Option<usize>,
    pub routes: Vec<Route>,
    /// Every manifest param's default (hidden ones included), by cell.
    pub cell_defaults: Vec<CellDefault>,
    /// The voice modulator's slots the DSP reads, with the input each feeds.
    #[serde(default)]
    pub modulators: Vec<ModulatorSpec>,
    /// Every modulation depth lane: which param it moves, from which slot.
    #[serde(default)]
    pub mod_targets: Vec<ModTargetSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModulatorSpec {
    /// 1-based slot.
    pub slot: usize,
    /// DSP input channel the slot's 0..1 output feeds.
    pub channel: usize,
    /// The ext input (1-4) the patch pinned the slot to with `(ext N)`.
    #[serde(default)]
    pub ext: Option<usize>,
}

/// One depth lane; indices are into [`PluginSpec::params`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModTargetSpec {
    pub base: usize,
    /// 1-based modulator slot.
    pub slot: usize,
    pub depth: usize,
    /// The destination's `__dgen_mod_active__` flag (set while any of its
    /// lanes has depth).
    #[serde(default)]
    pub active: Option<usize>,
    /// Depth range and unit in display units.
    pub depth_min: f32,
    pub depth_max: f32,
    #[serde(default)]
    pub unit: String,
    /// `base * 2^amount` instead of `base + amount`.
    #[serde(default)]
    pub octaves: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Route {
    pub channel: usize,
    pub signal: HostSignal,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CellDefault {
    pub cell: usize,
    pub span: usize,
    pub value: f32,
}

/// The per-sample host signals a DGen instrument input can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostSignal {
    Gate,
    Pitch,
    Velocity,
    Trigger,
    NoteOn,
    ClockPhase,
    ClockInc,
    Pressure,
    PitchBend,
    ModWheel,
    Slide,
    Legato,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamSpec {
    /// The DGen param name; the stable key for state and ids.
    pub name: String,
    pub label: String,
    /// Range and default in stored units (what the DSP cell holds).
    pub min: f32,
    pub max: f32,
    pub default: f32,
    #[serde(default)]
    pub unit: String,
    /// Stepped/enum params list their option labels (value = index).
    #[serde(default)]
    pub options: Vec<String>,
    /// A `%` param stored as a 0-1 ratio: shown and entered ×100.
    #[serde(default)]
    pub percent: bool,
    pub kind: ParamKind,
    pub target: ParamTarget,
    /// Panel placement: `main`, `mod` (a depth lane), `source` (a modulation
    /// source setting of slot `mod_slot`) or `hidden` (host plumbing).
    pub section: String,
    #[serde(default)]
    pub mod_slot: usize,
    /// Changing it restarts the slot's cycle on the bar (sync, division,
    /// retrigger settings).
    #[serde(default)]
    pub resync: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParamKind {
    Continuous,
    Enum,
    Boolean,
}

/// Where a param's value lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParamTarget {
    /// DGen state cells (the voice's DSP memory).
    Cell { cell: usize, span: usize },
    /// The voice modulator's state (`eseq-voice-mod` layout).
    Modulator { index: usize },
    /// A host control outside the DSP's state (the app's node header, e.g.
    /// `enabled`); the plugin keeps the value and writes it nowhere.
    Host,
}

impl ParamSpec {
    /// Shown to the host as an automatable param: the instrument's own and
    /// its depth lanes; modulation-source settings and plumbing are state
    /// only.
    pub fn host_visible(&self) -> bool {
        matches!(self.section.as_str(), "main" | "mod")
    }

    pub fn clamp(&self, value: f32) -> f32 {
        value.clamp(self.min.min(self.max), self.max.max(self.min))
    }

    /// The option `value` selects, for an enum param.
    pub fn option_label(&self, value: f32) -> Option<&str> {
        self.options.get(value.round().max(0.0) as usize).map(String::as_str)
    }

    /// Stored → display units.
    pub fn to_display(&self, value: f32) -> f32 {
        if self.percent { value * 100.0 } else { value }
    }

    /// Display → stored units.
    pub fn from_display(&self, value: f32) -> f32 {
        if self.percent { value / 100.0 } else { value }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UiSpec {
    /// Payload-relative content root the panel's Lisp loads from (`@/…`).
    pub root: String,
    /// Payload-relative panel source (the instrument's generated
    /// `custom-instrument-synth-ui`).
    pub entry: String,
    /// Payload-relative monospace font the cell grid is measured in.
    pub font: String,
    /// Panel size in points; 0 = measure the panel's layout at open.
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub theme: String,
}

/// FNV-1a of the param name: a param keeps its host id when params are added
/// or reordered by a re-export.
pub fn param_id(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // CLAP reserves 0xFFFFFFFF as CLAP_INVALID_ID.
    if hash == u32::MAX { 0x7fff_ffff } else { hash }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TensorInit {
    pub offset: usize,
    pub data: Vec<f32>,
}

/// The part of a DGen manifest the shell reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestTensors {
    #[serde(default)]
    tensor_init_data: Vec<TensorInit>,
}

/// A loaded payload: spec, the voice's initial state and resolved paths.
pub struct Payload {
    pub dir: PathBuf,
    pub spec: PluginSpec,
    /// One voice's state before its voice cell is set: param defaults and
    /// tensor data, plus the zeroed redzone.
    pub initial_state: Vec<f32>,
}

impl Payload {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let read = |path: &Path| std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
        let spec_path = dir.join(SPEC_FILE);
        let spec: PluginSpec =
            serde_json::from_slice(&read(&spec_path)?).map_err(|e| format!("{}: {e}", spec_path.display()))?;
        if spec.format != SPEC_FORMAT {
            return Err(format!("unsupported payload format {}", spec.format));
        }
        let manifest_path = dir.join(&spec.dsp.manifest);
        let tensors: ManifestTensors = serde_json::from_slice(&read(&manifest_path)?)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        let initial_state = initial_state(&spec.dsp.layout, &tensors.tensor_init_data);
        Ok(Self { dir: dir.to_path_buf(), spec, initial_state })
    }

    /// The payload dir of a bundle (`Foo.clap` / `Foo.vst3`), as handed to
    /// `clap_entry.init` by CLAP hosts and by clap-wrapper's VST3 entry.
    pub fn dir_for_bundle(bundle: &Path) -> PathBuf {
        bundle.join("Contents").join("Resources").join(PAYLOAD_DIR)
    }

    pub fn library_path(&self) -> PathBuf {
        self.dir.join(&self.spec.dsp.library)
    }
}

/// Zeroed state with the param defaults and tensor data written in.
pub fn initial_state(layout: &DspLayout, tensors: &[TensorInit]) -> Vec<f32> {
    let total = layout.total_memory_slots;
    let mut memory = vec![0.0f32; total + STATE_REDZONE_SLOTS];
    for default in &layout.cell_defaults {
        let end = (default.cell + default.span).min(total);
        if default.cell < end {
            memory[default.cell..end].fill(default.value);
        }
    }
    for tensor in tensors {
        let end = (tensor.offset + tensor.data.len()).min(total);
        if tensor.offset < end {
            memory[tensor.offset..end].copy_from_slice(&tensor.data[..end - tensor.offset]);
        }
    }
    memory
}
