//! Export an instrument as a CLAP / VST3 plugin bundle (docs/plugin-export-spec.md).
//!
//! The plugin binary is not built here: the prebuilt `eseq-plugin` shell
//! (`AppPaths::plugin_shell`) is copied into each bundle next to a payload —
//! the instrument's DGen library compiled for plugin use, its manifest, the
//! host-visible param table and the panel. The shell reads that payload at
//! load, so exporting needs only the DGen compiler the app already ships.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eseq_plugin_format::{
    CellDefault, DspLayout, DspSpec, HostSignal, ModTargetSpec, ModulatorSpec, PAYLOAD_DIR,
    ParamKind as PluginParamKind, ParamSpec, ParamTarget, PluginSpec, Route, SPEC_FILE, SPEC_FORMAT,
    UiSpec, DEFAULT_VOICES, VOICES_PARAM,
};

use crate::effects::panel_section::PanelSection;
use crate::effects::{EffectDescriptor, ModulationMode, ParamKind};
use crate::lisp_host::{DGenCompileKind, DGenManifest};

// The shell zeroes the same redzone after each voice's state as the app.
const _: () = assert!(crate::lisp_host::DGEN_STATE_REDZONE_SLOTS == eseq_plugin_format::STATE_REDZONE_SLOTS);

/// Scratch slices the exported DGen image is compiled with. Every instance of
/// one plugin shares the image, and each sounding voice needs its own slice,
/// so this bounds `instances × polyphony` per DAW process (spec §6). Untouched
/// slices cost only virtual address space.
pub const VOICE_SLOTS: usize = 64;
/// Voices one instance allocates (the voices param's ceiling).
const MAX_VOICES: usize = 16;
/// The compile-time sample rate is only a cache-key input; the shell passes
/// the host's rate at runtime.
const COMPILE_SAMPLE_RATE: u32 = 48_000;
const VENDOR: &str = "eseq";
const ID_PREFIX: &str = "com.universalsequences.eseq.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFormat {
    Clap,
    Vst3,
}

impl PluginFormat {
    pub const ALL: [PluginFormat; 2] = [PluginFormat::Clap, PluginFormat::Vst3];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Clap => "clap",
            Self::Vst3 => "vst3",
        }
    }

    /// The per-user install folder hosts scan.
    pub fn user_install_dir(self) -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let folder = match self {
            Self::Clap => "CLAP",
            Self::Vst3 => "VST3",
        };
        Some(home.join("Library/Audio/Plug-Ins").join(folder))
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "clap" => Some(Self::Clap),
            "vst3" | "vst" => Some(Self::Vst3),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Logical instrument name (`Drums/Latent Kick`) or an instrument folder.
    pub instrument: String,
    pub formats: Vec<PluginFormat>,
    /// Where bundles go; `None` installs into the per-user plug-in folders.
    pub out_dir: Option<PathBuf>,
    /// The most voices an instance may play (its voices param's ceiling);
    /// `None` = 16. Instances start at eseq's default of 6.
    pub polyphony: Option<usize>,
    /// Start from this preset's values (name or id).
    pub preset: Option<String>,
    /// Plugin defaults by param name, applied after the preset (e.g. the
    /// exporting track's current values).
    pub param_values: BTreeMap<String, f32>,
    /// Display name; `None` = the instrument's leaf name.
    pub name: Option<String>,
}

impl ExportOptions {
    pub fn new(instrument: impl Into<String>) -> Self {
        Self {
            instrument: instrument.into(),
            formats: PluginFormat::ALL.to_vec(),
            out_dir: None,
            polyphony: None,
            preset: None,
            param_values: BTreeMap::new(),
            name: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportReport {
    pub plugin_id: String,
    pub name: String,
    pub bundles: Vec<PathBuf>,
    pub params: usize,
    pub has_panel: bool,
}

/// A resolved instrument: its folder, DSP source and logical name.
struct InstrumentSource {
    dir: PathBuf,
    dsp: String,
    logical_name: String,
    leaf_name: String,
    /// The folder's path under its content root (differs from
    /// `logical_name` for a versioned release).
    content_rel: String,
}

fn resolve_instrument(target: &str) -> Result<InstrumentSource, String> {
    let as_path = Path::new(target);
    let dsp_path = if as_path.is_dir() {
        as_path.join("dsp.lisp")
    } else if as_path.is_file() {
        as_path.to_path_buf()
    } else {
        crate::lisp_host::instrument_source_path(target).map_err(|e| format!("instrument '{target}': {e}"))?
    };
    let dsp = std::fs::read_to_string(&dsp_path).map_err(|e| format!("{}: {e}", dsp_path.display()))?;
    let dir = dsp_path
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", dsp_path.display()))?
        .to_path_buf();
    // Where the folder sits under its content root: a versioned release's
    // `Drums/808 Kick/versions/1`. Panels read data files by this path.
    let content_rel = logical_name_for_dir(&dir);
    let logical_name = if as_path.exists() {
        content_rel
            .as_deref()
            .map(|rel| rel.split("/versions/").next().unwrap_or(rel).to_string())
            .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    } else {
        logical_instrument_name(target)
    };
    let leaf_name = logical_name.rsplit('/').next().unwrap_or(&logical_name).to_string();
    let content_rel = content_rel.unwrap_or_else(|| logical_name.clone());
    Ok(InstrumentSource { dir, dsp, logical_name, leaf_name, content_rel })
}

/// A track's instrument id without its tier or release pin
/// (`factory:Drums/Latent Kick@2` → `Drums/Latent Kick`), so the plugin id an
/// in-app export derives matches the CLI's for the same instrument.
fn logical_instrument_name(id: &str) -> String {
    let logical = match crate::app_paths::ContentTier::parse_id(id) {
        Ok(Some((_, path))) => path,
        _ => id.trim_end_matches('/'),
    };
    logical.split('@').next().unwrap_or(logical).to_string()
}

/// `content/instruments/Drums/Latent Kick` → `Drums/Latent Kick`.
fn logical_name_for_dir(dir: &Path) -> Option<String> {
    let dir = dir.canonicalize().ok()?;
    crate::app_paths::app_paths().instrument_roots().into_iter().find_map(|root| {
        let root = root.path.canonicalize().ok()?;
        let rel = dir.strip_prefix(&root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    })
}

pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// The CLAP id for an instrument. Derived from the logical name so re-exports
/// keep DAW sessions loading (clap-wrapper hashes it into the VST3 class id).
pub fn plugin_id_for(logical_name: &str) -> String {
    format!("{ID_PREFIX}{}", slug(logical_name))
}

fn is_drum(logical_name: &str) -> bool {
    logical_name.split('/').next().is_some_and(|c| c.eq_ignore_ascii_case("drums"))
}

/// The plugin's params: exactly the app's instrument descriptor, in its
/// order, so a `param-idx` the panel sends means what it means in eseq — the
/// instrument's own params, the voice modulator's source settings, then each
/// destination's active flag and depth lanes. Stored units.
fn param_specs(desc: &EffectDescriptor) -> Vec<ParamSpec> {
    use crate::instruments::voice_modulator::{MOD_PARAM_BASE, is_bar_resync_param};
    desc.params
        .iter()
        .map(|p| {
            let mut section = PanelSection::of(p);
            let header = crate::lisp_host::HEADER_SLOTS as u32;
            let target = if section == PanelSection::Source {
                ParamTarget::Modulator { index: (p.node_param_idx - MOD_PARAM_BASE) as usize }
            } else if p.node_param_idx < header || p.node_param_idx == u32::MAX {
                // The node header (enabled, …) or a host-only control: no DSP
                // cell. The plugin has no use for it; keep it out of sight.
                section = PanelSection::Hidden;
                ParamTarget::Host
            } else {
                ParamTarget::Cell {
                    cell: (p.node_param_idx - header) as usize,
                    span: p.node_param_span.max(1) as usize,
                }
            };
            let (kind, options) = match &p.kind {
                ParamKind::Continuous { .. } => (PluginParamKind::Continuous, Vec::new()),
                ParamKind::Boolean => (PluginParamKind::Boolean, Vec::new()),
                ParamKind::Enum { labels } => (PluginParamKind::Enum, labels.clone()),
            };
            let spec = ParamSpec {
                name: p.name.clone(),
                label: section.label(p),
                min: p.min,
                max: p.max,
                default: p.default,
                unit: p.kind.unit().unwrap_or_default().to_string(),
                options,
                percent: p.is_percent(),
                kind,
                target,
                section: section.name().to_string(),
                mod_slot: section.mod_slot(p),
                resync: is_bar_resync_param(p.node_param_idx),
            };
            ParamSpec { default: spec.clamp(spec.default), ..spec }
        })
        .collect()
}

/// The voice layout the shell runs, resolved by eseq's own manifest rules
/// (input routing, audio vs `@amp`/mod/probe outputs).
fn dsp_layout(manifest: &DGenManifest, desc: &EffectDescriptor, raw: &serde_json::Value) -> Result<DspLayout, String> {
    use crate::effects::gatepitch as gp;
    let signal = |output: usize| -> Option<HostSignal> {
        Some(match output {
            gp::OUTPUT_GATE => HostSignal::Gate,
            gp::OUTPUT_PITCH => HostSignal::Pitch,
            gp::OUTPUT_VELOCITY => HostSignal::Velocity,
            gp::OUTPUT_TRIGGER => HostSignal::Trigger,
            gp::OUTPUT_NOTE_ON => HostSignal::NoteOn,
            gp::OUTPUT_CLOCK_PHASE => HostSignal::ClockPhase,
            gp::OUTPUT_CLOCK_INC => HostSignal::ClockInc,
            gp::OUTPUT_PRESSURE => HostSignal::Pressure,
            gp::OUTPUT_PITCH_BEND => HostSignal::PitchBend,
            gp::OUTPUT_MOD_WHEEL => HostSignal::ModWheel,
            gp::OUTPUT_SLIDE => HostSignal::Slide,
            gp::OUTPUT_LEGATO => HostSignal::Legato,
            _ => return None,
        })
    };
    let routes = manifest
        .inputs
        .iter()
        .filter_map(|input| {
            let signal = signal(manifest.host_signal_output_for_input(input)?)?;
            Some(Route { channel: input.channel, signal })
        })
        .collect();
    let audio = manifest.audio_output_channels();
    let (&audio_left, audio_right) = audio.first().zip(Some(audio.get(1))).ok_or("the instrument has no audio output")?;
    Ok(DspLayout {
        total_memory_slots: manifest.total_memory_slots,
        voice_cell: manifest.voice_cell_id.filter(|&cell| cell < manifest.total_memory_slots),
        max_frames: raw["maxFrameCount"].as_u64().unwrap_or(crate::audio::engine::ENGINE_BLOCK_FRAMES as u64) as usize,
        n_inputs: manifest.n_inputs,
        n_outputs: manifest.n_outputs,
        audio_left,
        audio_right: audio_right.copied().unwrap_or(audio_left),
        amp_channel: manifest.amp_output_channel.filter(|&c| c < manifest.n_outputs),
        routes,
        cell_defaults: manifest
            .params
            .iter()
            .filter(|p| p.default != 0.0)
            .map(|p| CellDefault { cell: p.cell_id, span: p.cell_span, value: p.default })
            .collect(),
        modulators: manifest
            .modulators
            .iter()
            .map(|m| ModulatorSpec { slot: m.slot, channel: m.input_channel, ext: m.ext })
            .collect(),
        mod_targets: desc
            .instrument_modulation_targets
            .iter()
            .map(|t| ModTargetSpec {
                base: t.base_param_idx,
                slot: t.modulator_slot,
                depth: t.depth_param_idx,
                active: t.active_param_idx,
                depth_min: t.depth_min,
                depth_max: t.depth_max,
                unit: t.depth_unit.clone().unwrap_or_default(),
                octaves: t.mod_mode == ModulationMode::Octaves,
            })
            .collect(),
    })
}

/// Compile the instrument for plugin use into `dir` (`instrument.<ext>` +
/// `instrument.json`, the host manifest with `tensorInitData`) and parse it.
fn compile_for_plugin(source: &InstrumentSource, dir: &Path) -> Result<(EffectDescriptor, DspLayout), String> {
    let effective = crate::lisp_host::effective_dgen_source(DGenCompileKind::Instrument, &source.dsp, COMPILE_SAMPLE_RATE)?;
    let json = crate::lisp_host::compile_effective_dgen_source_to_dir_with_voices(
        DGenCompileKind::Instrument,
        &effective,
        COMPILE_SAMPLE_RATE,
        Some(&source.dir),
        dir,
        "instrument",
        VOICE_SLOTS,
    )?;
    crate::lisp_host::audit_dylib(&dir.join(library_file()))?;
    let manifest = crate::lisp_host::parse_manifest(&json)?;
    let raw: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("compiled manifest: {e}"))?;
    let desc = crate::lisp_host::instrument_descriptor_from_manifest(&source.logical_name, &manifest);
    let layout = dsp_layout(&manifest, &desc, &raw)?;
    Ok((desc, layout))
}

fn library_file() -> String {
    format!("instrument.{}", crate::lisp_host::DGEN_SHARED_LIBRARY_EXTENSION)
}

fn info_plist(spec: &PluginSpec, executable: &str, format: PluginFormat) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>English</string>
	<key>CFBundleExecutable</key>
	<string>{executable}</string>
	<key>CFBundleIdentifier</key>
	<string>{id}.{ext}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundlePackageType</key>
	<string>BNDL</string>
	<key>CFBundleSignature</key>
	<string>????</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#,
        executable = escape(executable),
        id = escape(&spec.id),
        ext = format.extension(),
        name = escape(&spec.name),
        version = escape(&spec.version),
    )
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
    }
    Ok(())
}

/// Ad-hoc sign: inner DSP library first, then the bundle. Exporting rewrites
/// the bundle, so any release signature on the shell cannot survive; ad-hoc
/// is what hosts on this Mac need (spec §11).
fn codesign(bundle: &Path, inner: &[PathBuf]) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let sign = |path: &Path| -> Result<(), String> {
            let output = std::process::Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(path)
                .output()
                .map_err(|e| format!("codesign: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "codesign {}: {}",
                    path.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(())
        };
        for path in inner {
            sign(path)?;
        }
        sign(bundle)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (bundle, inner);
    Ok(())
}

/// Assemble the payload directory (everything under `eseq-payload/`).
fn build_payload(
    source: &InstrumentSource,
    options: &ExportOptions,
    payload: &Path,
) -> Result<PluginSpec, String> {
    let dsp_dir = payload.join("dsp");
    let (desc, layout) = compile_for_plugin(source, &dsp_dir)?;
    // Only the library and the host manifest ship; drop compile by-products.
    let library = library_file();
    for entry in std::fs::read_dir(&dsp_dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let keep = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == library || n == "instrument.json");
        if !keep {
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }

    let mut params = param_specs(&desc);
    let max_voices = options.polyphony.unwrap_or(MAX_VOICES).clamp(1, VOICE_SLOTS);
    params.push(ParamSpec {
        name: VOICES_PARAM.into(),
        label: "voices".into(),
        min: 1.0,
        max: max_voices as f32,
        default: DEFAULT_VOICES.min(max_voices) as f32,
        unit: String::new(),
        options: Vec::new(),
        percent: false,
        kind: PluginParamKind::Continuous,
        target: ParamTarget::Host,
        section: "hidden".into(),
        mod_slot: 0,
        resync: false,
    });
    if params.is_empty() {
        return Err("the instrument has no visible params".into());
    }
    let presets = crate::lisp_host::load_instrument_presets(&source.logical_name).unwrap_or_default();
    let mut values: BTreeMap<String, f32> = BTreeMap::new();
    if let Some(preset) = &options.preset {
        let found = presets
            .iter()
            .find(|p| p.id == *preset || p.name.eq_ignore_ascii_case(preset))
            .ok_or_else(|| format!("no preset '{preset}' for '{}'", source.logical_name))?;
        values.extend(found.params.clone());
    }
    values.extend(options.param_values.clone());
    for param in &mut params {
        if let Some(&value) = values.get(&param.name) {
            param.default = param.clamp(value);
        }
    }

    // GPL corresponding source + presets, for provenance and later reuse.
    let source_dir = payload.join("source");
    std::fs::create_dir_all(&source_dir).map_err(|e| e.to_string())?;
    for file in ["dsp.lisp", "ui.lisp", "instrument.json"] {
        let from = source.dir.join(file);
        match std::fs::copy(&from, source_dir.join(file)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", from.display())),
            _ => {}
        }
    }
    if !presets.is_empty() {
        let json = serde_json::to_vec_pretty(&presets).map_err(|e| e.to_string())?;
        std::fs::write(payload.join("presets.json"), json).map_err(|e| e.to_string())?;
    }

    let drum = is_drum(&source.logical_name);
    let name = options.name.clone().unwrap_or_else(|| source.leaf_name.clone());
    let features = if drum {
        vec!["instrument", "drum", "stereo"]
    } else {
        vec!["instrument", "synthesizer", "stereo"]
    };
    let spec = PluginSpec {
        format: SPEC_FORMAT,
        id: plugin_id_for(&source.logical_name),
        name,
        vendor: VENDOR.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: format!("eseq instrument {}", source.logical_name),
        url: "https://github.com/universalsequences/eseq".into(),
        features: features.into_iter().map(str::to_string).collect(),
        dsp: DspSpec {
            library: format!("dsp/{library}"),
            manifest: "dsp/instrument.json".into(),
            voice_slots: VOICE_SLOTS,
            layout,
        },
        polyphony: max_voices,
        params,
        ui: build_ui_payload(source, payload)?,
        source_instrument: source.logical_name.clone(),
    };
    let json = serde_json::to_vec_pretty(&spec).map_err(|e| e.to_string())?;
    std::fs::write(payload.join(SPEC_FILE), json).map_err(|e| e.to_string())?;
    Ok(spec)
}

/// The panel half of the payload: the eseqlisp UI runtime the panel renders
/// through (`content/ui` + `content/core`), the instrument folder at its
/// content-relative path (panels read data files such as `data/family.json`
/// by that path), the monospace font, and the panel source generated for this
/// one instrument. An instrument with no `ui.lisp` gets eseq's own fallback,
/// the generic param grid, exactly as the app shows it.
fn build_ui_payload(source: &InstrumentSource, payload: &Path) -> Result<Option<UiSpec>, String> {
    // Every instrument gets a panel; `Option` keeps the format open to
    // headless exports.
    let ui_path = source.dir.join("ui.lisp");
    let panel = match std::fs::read_to_string(&ui_path) {
        Ok(ui_src) => crate::custom_ui_codegen::single_instrument_ui_source(
            &source.logical_name,
            &ui_path.display().to_string(),
            &ui_src,
        )?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => NO_CUSTOM_UI.to_string(),
        Err(e) => return Err(format!("{}: {e}", ui_path.display())),
    };
    let paths = crate::app_paths::app_paths();
    let factory = paths.factory_root();
    let ui_dir = payload.join("ui");
    let content = ui_dir.join("content");
    for dir in UI_RUNTIME_DIRS {
        copy_dir(&factory.join(dir), &content.join(dir))?;
    }
    let instrument_rel = Path::new("instruments").join(&source.content_rel);
    crate::package_export::copy_dir_filtered(&source.dir, &content.join(&instrument_rel))?;
    let font = paths.ui_monospace_font();
    let fonts = ui_dir.join("fonts");
    std::fs::create_dir_all(&fonts).map_err(|e| e.to_string())?;
    let font_name = font.file_name().ok_or("monospace font has no file name")?;
    std::fs::copy(&font, fonts.join(font_name)).map_err(|e| format!("{}: {e}", font.display()))?;
    std::fs::write(ui_dir.join("panel.lisp"), panel).map_err(|e| e.to_string())?;
    Ok(Some(UiSpec {
        root: "ui/content".into(),
        entry: "ui/panel.lisp".into(),
        font: format!("ui/fonts/{}", font_name.to_string_lossy()),
        // Measured by the shell from the panel's layout when it opens.
        width: 0.0,
        height: 0.0,
        theme: DEFAULT_THEME.into(),
    }))
}

/// The factory content folders a panel's Lisp loads from (`@/ui/…`,
/// `eseq.kinds` and the other core modules).
const UI_RUNTIME_DIRS: [&str; 2] = ["ui", "core"];
const DEFAULT_THEME: &str = "seq-theme-mac-osx-dark";
/// Panel source for an instrument without `ui.lisp`: declining the custom UI
/// makes `instrument-synth-panel-body` draw the param grid.
const NO_CUSTOM_UI: &str = "(def custom-instrument-synth-ui (inst) false)\n";

pub fn export_instrument_plugin(options: &ExportOptions) -> Result<ExportReport, String> {
    if options.formats.is_empty() {
        return Err("no plugin format selected".into());
    }
    let shell = crate::app_paths::app_paths().plugin_shell();
    let source = resolve_instrument(&options.instrument)?;
    let staging = crate::app_paths::app_paths().dgen_scratch_dir().join("plugin-export").join(format!(
        "{}-{}-{}",
        slug(&source.logical_name),
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
    ));
    let payload = staging.join(PAYLOAD_DIR);
    std::fs::create_dir_all(&payload).map_err(|e| format!("{}: {e}", payload.display()))?;
    let result = (|| {
        let spec = build_payload(&source, options, &payload)?;
        let mut bundles = Vec::new();
        for &format in &options.formats {
            let out_dir = match &options.out_dir {
                Some(dir) => dir.clone(),
                None => format.user_install_dir().ok_or("no HOME to install into")?,
            };
            bundles.push(write_bundle(&spec, &shell, &payload, &out_dir, format)?);
        }
        Ok(ExportReport {
            plugin_id: spec.id.clone(),
            name: spec.name.clone(),
            bundles,
            params: spec.params.len(),
            has_panel: spec.ui.is_some(),
        })
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn write_bundle(
    spec: &PluginSpec,
    shell: &Path,
    payload: &Path,
    out_dir: &Path,
    format: PluginFormat,
) -> Result<PathBuf, String> {
    let file_name = format!("{}.{}", sanitize_file_name(&spec.name), format.extension());
    let bundle = out_dir.join(&file_name);
    // Build beside the target, then swap in, so a host scanning mid-export
    // never sees a half-written bundle.
    let building = out_dir.join(format!(".{file_name}.building"));
    let _ = std::fs::remove_dir_all(&building);
    let contents = building.join("Contents");
    let macos = contents.join("MacOS");
    std::fs::create_dir_all(&macos).map_err(|e| format!("{}: {e}", macos.display()))?;
    let executable = sanitize_file_name(&spec.name);
    std::fs::copy(shell, macos.join(&executable)).map_err(|e| {
        format!("plugin shell {}: {e} (dev: `cargo build -p eseq-plugin --release`)", shell.display())
    })?;
    std::fs::write(contents.join("Info.plist"), info_plist(spec, &executable, format)).map_err(|e| e.to_string())?;
    std::fs::write(contents.join("PkgInfo"), "BNDL????").map_err(|e| e.to_string())?;
    let resources_payload = contents.join("Resources").join(PAYLOAD_DIR);
    copy_dir(payload, &resources_payload)?;
    codesign(&building, &[resources_payload.join(&spec.dsp.library)])?;
    match std::fs::remove_dir_all(&bundle) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("replace {}: {e}", bundle.display()));
        }
        _ => {}
    }
    std::fs::rename(&building, &bundle).map_err(|e| format!("{}: {e}", bundle.display()))?;
    Ok(bundle)
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | ':' | '\\' | '\0') { '-' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    if trimmed.is_empty() { "eseq instrument".into() } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_and_release_pins_do_not_change_the_logical_name() {
        assert_eq!(logical_instrument_name("factory:Drums/Latent Kick@2"), "Drums/Latent Kick");
        assert_eq!(logical_instrument_name("Drums/Latent Kick/"), "Drums/Latent Kick");
    }

    #[test]
    fn plugin_ids_are_stable_slugs() {
        assert_eq!(plugin_id_for("Drums/Latent Kick Morph"), "com.universalsequences.eseq.drums-latent-kick-morph");
        assert_eq!(slug("  Hello,  World!! "), "hello-world");
    }

    #[test]
    fn plugin_params_are_the_app_descriptor_in_order() {
        let manifest = crate::lisp_host::parse_manifest(
            r#"{ "processAbi": "dgen-host-abi-v1", "totalMemorySlots": 20, "params": [
                { "name": "z1", "cellId": 0, "min": -3, "max": 3, "default": 0.5 },
                { "name": "__mod__z1__active", "hidden": true, "cellId": 1, "min": 0, "max": 1, "default": 0 },
                { "name": "__mod__z1__depth__slot1", "hidden": true, "cellId": 2, "min": -6, "max": 6, "default": 0 },
                { "name": "mix", "unit": "%", "cellId": 3, "min": 0, "max": 1, "default": 0.25 }
            ], "modDestinations": [{ "name": "z1", "paramCellId": 0, "activeCellId": 1, "mode": "additive",
                "min": -3, "max": 3, "depthMin": -6, "depthMax": 6,
                "depthLanes": [{ "slot": 1, "depthCellId": 2 }] }],
              "modulators": [{ "inputChannel": 0, "name": "mod1", "slot": 1 }],
              "inputs": [{ "channel": 0, "name": "mod1" }], "outputs": [{ "channel": 0, "name": "out" }] }"#,
        )
        .expect("manifest");
        let desc = crate::lisp_host::instrument_descriptor_from_manifest("Test", &manifest);
        let specs = param_specs(&desc);
        assert_eq!(specs.len(), desc.params.len());
        assert_eq!((specs[0].name.as_str(), specs[0].section.as_str()), ("z1", "main"));
        assert!(specs[1].percent);
        let source = specs.iter().find(|p| p.name == "mod1_source").expect("slot 1 type");
        assert_eq!((source.section.as_str(), source.label.as_str(), source.mod_slot), ("source", "type", 1));
        assert!(matches!(source.target, ParamTarget::Modulator { .. }));
        let depth = specs.iter().find(|p| p.section == "mod").expect("a depth lane");
        assert_eq!(depth.target, ParamTarget::Cell { cell: 2, span: 1 });
        assert!(specs.iter().any(|p| p.section == "hidden" && p.target == ParamTarget::Cell { cell: 1, span: 1 }));
        // The node header's controls (enabled) never land in DSP memory.
        for (spec, param) in specs.iter().zip(&desc.params) {
            if param.node_param_idx < crate::lisp_host::HEADER_SLOTS as u32 {
                assert_eq!(spec.target, ParamTarget::Host, "{}", spec.name);
            }
        }
    }
}
