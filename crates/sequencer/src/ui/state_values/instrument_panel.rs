use super::*;

/// Where a param sits on its device's panel: the main controls, a
/// modulation lane's own params (`mod …`), a modulation source's settings
/// (voice modulator source params), or host plumbing nobody shows. Shared by
/// the instrument panel builders and the host kinds' `param.section`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelSection {
    Main,
    Mod,
    Source,
    Hidden,
}

impl PanelSection {
    pub(crate) fn of(pdesc: &sequencer::effects::ParamDescriptor) -> Self {
        if is_generated_host_mod_param(&pdesc.name) || is_hidden_dgen_mod_param(&pdesc.name) {
            Self::Hidden
        } else if is_source_param(pdesc.node_param_idx) {
            Self::Source
        } else if is_mod_param(&pdesc.name) {
            Self::Mod
        } else {
            Self::Main
        }
    }

    /// Where param `index` of `desc` sits on `device`'s panel. An effect's
    /// (a track chain, bus or drum rack slot effect) has no `mod` split: it
    /// lists every param (main) but its voice-modulator source settings
    /// (source; a host-routed sidechain param stays main), its modulation
    /// routing (a lane's depth, source and switch params) and the host
    /// plumbing (both hidden). Any other device's is [`Self::of`].
    pub(crate) fn of_device(
        device: DeviceSlot,
        desc: &sequencer::effects::EffectDescriptor,
        index: usize,
    ) -> Self {
        let Some(pdesc) = desc.params.get(index) else {
            return Self::Hidden;
        };
        if !device.is_effect() {
            return Self::of(pdesc);
        }
        let sidechain = matches!(
            pdesc.host_control,
            Some(sequencer::effects::HostControl::FxSidechain { .. })
        );
        if is_generated_host_mod_param(&pdesc.name) || is_hidden_dgen_mod_param(&pdesc.name) {
            Self::Hidden
        } else if sequencer::instruments::voice_modulator::is_source_param(pdesc.node_param_idx)
            && !sidechain
        {
            // A host-only control (u32::MAX) is no modulation source's.
            match pdesc.node_param_idx {
                u32::MAX => Self::Hidden,
                _ => Self::Source,
            }
        } else if modulation_routing_param_indices(desc).contains(&index) {
            Self::Hidden
        } else {
            Self::Main
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Mod => "mod",
            Self::Source => "source",
            Self::Hidden => "hidden",
        }
    }

    /// The name the panel shows: a mod param without its `mod ` prefix, a
    /// source param by its role (`type`, `rate`, …).
    pub(crate) fn label(self, pdesc: &sequencer::effects::ParamDescriptor) -> String {
        match self {
            Self::Mod => (pdesc.name.strip_prefix("mod ").unwrap_or(&pdesc.name)).to_string(),
            Self::Source => rename_source_param(&pdesc.name),
            Self::Main | Self::Hidden => pdesc.name.clone(),
        }
    }

    /// The modulation source (1-based) a source param sets; 0 otherwise.
    pub(crate) fn mod_slot(self, pdesc: &sequencer::effects::ParamDescriptor) -> usize {
        match self {
            Self::Source => {
                sequencer::instruments::voice_modulator::slot_from_param_name(&pdesc.name)
                    .unwrap_or(0)
            }
            _ => 0,
        }
    }
}

fn is_mod_param(name: &str) -> bool {
    name.starts_with("mod ")
}

fn is_generated_host_mod_param(name: &str) -> bool {
    name.starts_with("__host_mod__")
}

fn is_hidden_dgen_mod_param(name: &str) -> bool {
    name.starts_with("__dgen_mod_active__")
}

fn is_source_param(node_param_idx: u32) -> bool {
    // u32::MAX marks host-only controls such as sampler slicing; it is not a
    // packed voice-modulator source index.
    node_param_idx != u32::MAX
        && sequencer::instruments::voice_modulator::is_source_param(node_param_idx)
}

fn rename_source_param(name: &str) -> String {
    sequencer::instruments::voice_modulator::source_param_display_name(name)
}

/// An instrument's visible key locks: per param, `(note, value)` in display
/// units, ascending by note; and the notes holding any, ascending. A lock
/// whose node id no longer matches its param's (a rebuilt instrument) is
/// not shown. Shared by the instrument panel and the host kinds'
/// `param.key-locks` / `device.key-locked-notes`.
pub(crate) struct InstrumentKeyLocks {
    pub(crate) by_param: Vec<Vec<(u8, f32)>>,
    pub(crate) notes: Vec<u8>,
}

pub(crate) fn instrument_key_locks(
    slot: &sequencer::effects::EffectSlotState,
    params: &[sequencer::effects::ParamDescriptor],
) -> InstrumentKeyLocks {
    let slot_num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let mut by_param = vec![Vec::<(u8, f32)>::new(); params.len()];
    let mut notes = Vec::<u8>::new();
    for note in 0..sequencer::effects::MAX_MIDI_NOTES {
        let note = note as u8;
        if !slot.key_locks.note_has_any_lock(note, slot_num_params) {
            continue;
        }
        for (param_idx, pdesc) in params.iter().enumerate().take(slot_num_params) {
            let Some(value) = slot.key_locks.get(note, param_idx) else {
                continue;
            };
            if slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx) {
                continue;
            }
            by_param[param_idx].push((note, pdesc.stored_to_user(value)));
            if notes.last() != Some(&note) {
                notes.push(note);
            }
        }
    }
    InstrumentKeyLocks { by_param, notes }
}

/// Slice sensitivity as the sampler panel resolves it. Marker indices in
/// `edit-sampler-slice` payloads address the list this panel rendered, so the
/// host command has to reach the same value — p-lock and descriptor-tail
/// fallback included — or an edit lands on the wrong marker.
///
/// A sampler track can be backed by a descriptor that predates the slice
/// controls (stale/converted tracks in particular), so the default resolves
/// through `get` and slicing is skipped entirely when the tail is absent.
fn sampler_slice_param_value(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::EffectDescriptor,
    plock_step: Option<usize>,
    param_idx: usize,
) -> Option<f32> {
    let default = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
        Some(slot.defaults.get(param_idx))
    } else {
        desc.params.get(param_idx).map(|param| param.default)
    };
    default.map(|default| {
        plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default)
    })
}

pub(crate) fn sampler_slice_mode(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::EffectDescriptor,
    plock_step: Option<usize>,
) -> Option<f32> {
    sampler_slice_param_value(
        slot,
        desc,
        plock_step,
        sequencer::instruments::sampler::SLOT_PARAM_SLICE_MODE,
    )
}

pub(crate) fn sampler_slice_sensitivity(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::EffectDescriptor,
    plock_step: Option<usize>,
) -> Option<f32> {
    sampler_slice_param_value(
        slot,
        desc,
        plock_step,
        sequencer::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY,
    )
}

/// The playback start and end a track sampler shows at `step` (stored,
/// 0-1 of the sample): the step's p-lock, else its own. Shared by the
/// sampler panel, its selection-time fields and the host kinds'
/// `device.start-time` / `end-time`.
pub(crate) fn sampler_selection(
    slot: &sequencer::effects::EffectSlotState,
    step: Option<usize>,
) -> (f32, f32) {
    let at = |param| {
        step.and_then(|step| slot.plocks.get(step, param))
            .unwrap_or_else(|| slot.defaults.get(param))
    };
    use sequencer::instruments::sampler::{SLOT_PARAM_END, SLOT_PARAM_START};
    (at(SLOT_PARAM_START), at(SLOT_PARAM_END))
}

/// The registered sample a sampler's waveform draws, loading (and
/// registering) it on first use; `None` without a path or when it fails to
/// load (reported). Shared by the sampler panels and the host kinds'
/// `device.sample-buffer`.
pub(crate) fn sampler_waveform_sample(
    path: Option<&Path>,
    what: &str,
) -> Option<Arc<eseqlisp::audio::sample::SampleBuffer>> {
    let path = path?;
    match load_waveform_sample(path) {
        Ok(sample) => Some(sample),
        Err(error) => {
            eprintln!(
                "{what}: failed to register sample {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// A sampler's slice markers in seconds, every candidate, and whether the
/// slice `sensitivity` keeps each (1 or 0, as `slice-active` reads them;
/// with `edits` applied); none outside slice mode (`slice_mode` 1) or
/// before the sample is analysed. Shared by the sampler panels and the host
/// kinds' `device.slices` / `slice-active`.
pub(crate) fn sampler_slices(
    app: &app::App,
    buffer_id: i32,
    slice_mode: f32,
    sensitivity: f32,
    edits: Option<&sequencer::analysis::SamplerSliceEdits>,
) -> (Vec<f64>, Vec<f64>) {
    if slice_mode.round() != 1.0 {
        return (Vec::new(), Vec::new());
    }
    let Some(table) = app.sample_analysis.cache().table(buffer_id) else {
        return (Vec::new(), Vec::new());
    };
    // Sensitivity deactivates markers rather than removing them, so the
    // panel renders every candidate and carries a parallel active flag.
    let (frames, active) = table.with_edits(edits).slice_markers(sensitivity);
    let rate = table.sample_rate.max(1) as f64;
    let seconds = frames
        .into_iter()
        .map(|frame| frame as f64 / rate)
        .collect();
    let active = (active.into_iter())
        .map(|on| if on { 1.0 } else { 0.0 })
        .collect();
    (seconds, active)
}

/// A sample's analysis as the sampler panel shows it.
pub(crate) struct SamplerAnalysis {
    /// `none`, `pending`, `ready` or `failed`.
    pub(crate) status: &'static str,
    pub(crate) message: String,
    /// (bpm, confidence) once ready.
    pub(crate) tempo: Option<(f64, f64)>,
    /// The first downbeat and the onsets, in seconds.
    pub(crate) downbeat: Option<f64>,
    pub(crate) onsets: Vec<f64>,
}

impl SamplerAnalysis {
    /// Buffer `buffer_id`'s analysis (the cache's entry). Shared by the
    /// sampler panel and the host kinds' `device.analysis-*`.
    pub(crate) fn of(app: &app::App, buffer_id: i32) -> Self {
        use sequencer::analysis::AnalysisEntry;
        let rate = app.graph.sample_rate.max(1) as f64;
        let mut analysis = Self {
            status: "none",
            message: String::new(),
            tempo: None,
            downbeat: None,
            onsets: Vec::new(),
        };
        let Some(entry) = app.sample_analysis.cache().get(buffer_id) else {
            return analysis;
        };
        match entry.as_ref() {
            AnalysisEntry::Pending => {
                analysis.status = "pending";
                analysis.message = "Analyzing...".to_string();
            }
            AnalysisEntry::Ready(result) => {
                analysis.status = "ready";
                analysis.message = format!("{:.1} BPM", result.bpm);
                analysis.tempo = Some((result.bpm as f64, result.bpm_confidence as f64));
                analysis.downbeat = result.downbeat_frame.map(|frame| frame as f64 / rate);
                analysis.onsets = (result.onsets_frames.iter())
                    .map(|frame| *frame as f64 / rate)
                    .collect();
            }
            AnalysisEntry::Failed(error) => {
                analysis.status = "failed";
                analysis.message = error.clone();
            }
        }
        analysis
    }
}

/// The name an instrument panel shows for `track`'s instrument (its
/// `name`): the engine's, else Modulator or Instrument.
pub(crate) fn instrument_panel_name(app: &app::App, track: usize) -> String {
    let modulator = app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Modulator);
    current_custom_instrument_name(app, track).unwrap_or_else(|| {
        if modulator {
            "Modulator".to_string()
        } else {
            "Instrument".to_string()
        }
    })
}

/// The instrument panel header's `display-name` for `track`'s instrument:
/// a drum rack's track name (Rack when none), Sampler for a sampler (whose
/// panel carries none), else the panel's name without its folder or pin.
/// Shared with the rack panel and the host kinds' `device.display-name`.
pub(crate) fn instrument_panel_display_name(app: &app::App, track: usize) -> String {
    use sequencer::sequencer::InstrumentType;
    match app.graph.track_instrument_types.get(track) {
        Some(InstrumentType::Rack) => (app.tracks.get(track))
            .map(|name| instrument_display_name(name))
            .unwrap_or_else(|| "Rack".to_string()),
        Some(InstrumentType::Sampler) => "Sampler".to_string(),
        _ => instrument_display_name(&instrument_panel_name(app, track)),
    }
}

