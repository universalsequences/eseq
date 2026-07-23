use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::lisp_host::{self, DGenManifest, LoadedDGenLib};
use crate::sequencer::{
    rack_slot_pool_index, BusId, CustomInstrumentRunMode, InstrumentSlotResetSummary,
    InstrumentType, ModDestination, RackRouting, RackSlotParamPlocks, RackSlotSnapshot,
    RackTrackSnapshot, TrackOutput, TrackSoundState, DRUM_RACK_FIRST_PAD_NOTE,
    DRUM_RACK_LAST_PAD_NOTE, DRUM_RACK_TOTAL_PAD_NOTES, EXT_MOD_INPUT_COUNT, MAX_RACK_SLOTS,
    MAX_SAMPLER_POOLS, MAX_TRACKS,
};
use crate::audio::MAX_VOICES;

use super::fx_chain::{
    connect_fx_chain_gap, connect_fx_chain_host, rewire_fx_chain, ChainSuccessor, FxChainHost,
    FxChainLocator, FxChainSlotView, FxLeaseSlotRemoval, StereoEndpoint,
};
use super::{App, EngineDescriptor, EngineNodeIds, RackSlotNodeIds, TrackNodeIds};

const DELETE_WITHOUT_SHIFT_ENV: &str = "TINYSEQ_DELETE_TRACK_WITHOUT_SHIFT";

fn graph_node_id_to_slot_identity(node_id: i32) -> u32 {
    u32::try_from(node_id).unwrap_or(0)
}

fn first_graph_node_identity(ids: &[i32]) -> u32 {
    ids.first()
        .copied()
        .map(graph_node_id_to_slot_identity)
        .unwrap_or(0)
}

const DEFAULT_LAYER_SLOT_MAX_POLYPHONY: usize = 4;
const DEFAULT_DRUM_SLOT_MAX_POLYPHONY: usize = 1;
const RACK_TEARDOWN_TAIL: Duration = Duration::from_secs(8);
const MAX_DEFERRED_RACK_TEARDOWNS: usize = 16;

/// Structural identity of a rack's live audio graph. Two racks with equal
/// signatures can share the same graph nodes; fields omitted from this type
/// are scene parameters that can be applied to those nodes in place.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RackTopologySignature {
    pub slots: Vec<RackSlotTopologySignature>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RackSlotTopologySignature {
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    /// Source identity for custom/modulator slots; samplers have no structural
    /// source identity because their buffers can be replaced in place.
    pub engine_id: Option<usize>,
    /// The exact slot sequence consumed by `connect_fx_chain_host`.
    pub fx_chain: Vec<(u32, u32, usize, usize)>,
}

fn rack_topology_signature(rack: &RackTrackSnapshot) -> RackTopologySignature {
    RackTopologySignature {
        slots: rack
            .slots
            .iter()
            .map(|slot| RackSlotTopologySignature {
                instrument_type: slot.instrument_type,
                instrument_run_mode: slot.instrument_run_mode,
                engine_id: match slot.instrument_type {
                    InstrumentType::Custom | InstrumentType::Modulator => {
                        slot.track_sound_state.engine_id
                    }
                    InstrumentType::Sampler | InstrumentType::Rack => None,
                },
                fx_chain: slot
                    .effect_slots
                    .iter()
                    .zip(&slot.effect_descriptors)
                    .map(|(effect, descriptor)| {
                        (
                            effect.node_id,
                            effect.modulator_node_id,
                            descriptor.input_channels,
                            descriptor.output_channels,
                        )
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub struct DeferredEngineRouteGeneration {
    engine_id: usize,
    node_ids: Vec<i32>,
}

pub struct DeferredRackTeardown {
    slots: Vec<RackSlotNodeIds>,
    engine_routes: Vec<DeferredEngineRouteGeneration>,
    track_idx: usize,
    due_at: Instant,
}

fn appended_rack_slot_max_polyphony(_existing_slots: &[RackSlotSnapshot]) -> usize {
    DEFAULT_LAYER_SLOT_MAX_POLYPHONY.min(MAX_VOICES).max(1)
}

fn validate_drum_rack_pad_note(pad_note: i32) -> bool {
    (DRUM_RACK_FIRST_PAD_NOTE..=DRUM_RACK_LAST_PAD_NOTE).contains(&pad_note)
}

fn validate_rack_slot_pad_map(
    routing: RackRouting,
    slots: &[RackSlotSnapshot],
) -> Result<(), String> {
    if routing != RackRouting::ByPitch {
        return Ok(());
    }

    let mut occupied = [false; DRUM_RACK_TOTAL_PAD_NOTES];
    for (slot_idx, slot) in slots.iter().enumerate() {
        let Some(pad_note) = slot.pad_note else {
            return Err(format!(
                "Drum rack slot {} is missing a pad note",
                slot_idx + 1
            ));
        };
        if !validate_drum_rack_pad_note(pad_note) {
            return Err(format!(
                "Drum rack slot {} has unsupported pad note {}",
                slot_idx + 1,
                pad_note
            ));
        }
        let pad_idx = (pad_note - DRUM_RACK_FIRST_PAD_NOTE) as usize;
        if occupied[pad_idx] {
            return Err(format!("Drum rack pad {pad_note} is already occupied"));
        }
        occupied[pad_idx] = true;
    }
    Ok(())
}

fn validate_rack_build_slot_pad_map(
    routing: RackRouting,
    slots: &[RackSlotBuildSpec<'_>],
) -> Result<(), String> {
    if routing != RackRouting::ByPitch {
        return Ok(());
    }

    let mut occupied = [false; DRUM_RACK_TOTAL_PAD_NOTES];
    for (slot_idx, slot) in slots.iter().enumerate() {
        let Some(pad_note) = slot.pad_note else {
            return Err(format!(
                "Drum rack slot {} is missing a pad note",
                slot_idx + 1
            ));
        };
        if !validate_drum_rack_pad_note(pad_note) {
            return Err(format!(
                "Drum rack slot {} has unsupported pad note {}",
                slot_idx + 1,
                pad_note
            ));
        }
        let pad_idx = (pad_note - DRUM_RACK_FIRST_PAD_NOTE) as usize;
        if occupied[pad_idx] {
            return Err(format!("Drum rack pad {pad_note} is already occupied"));
        }
        occupied[pad_idx] = true;
    }
    Ok(())
}

fn preserve_rack_slot_configuration(
    mut replacement: RackSlotSnapshot,
    existing: &RackSlotSnapshot,
) -> RackSlotSnapshot {
    replacement.pad_note = existing.pad_note;
    replacement.choke_group = existing.choke_group;
    replacement.instrument_base_note_offset = existing.instrument_base_note_offset;
    replacement.gain = existing.gain;
    replacement.pan = existing.pan;
    replacement.mute = existing.mute;
    replacement.solo = existing.solo;
    replacement.max_polyphony = existing.max_polyphony;
    replacement.param_plocks = existing.param_plocks.clone();
    replacement.effect_slots = existing.effect_slots.clone();
    replacement.effect_descriptors = existing.effect_descriptors.clone();
    replacement.custom_effect_names = existing.custom_effect_names.clone();
    replacement
}

fn instrument_display_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}

fn custom_route_parent_track(route_idx: usize) -> Option<usize> {
    if route_idx < MAX_TRACKS {
        Some(route_idx)
    } else if route_idx < crate::sequencer::MAX_SAMPLER_POOLS {
        Some((route_idx - MAX_TRACKS) / MAX_RACK_SLOTS)
    } else {
        None
    }
}

fn free_patch_idle_route_value(
    route_track: usize,
    target_track: usize,
    transport_playing: bool,
) -> f32 {
    if transport_playing && route_track == target_track {
        1.0
    } else {
        0.0
    }
}

struct TrackShell {
    voice_sum_id: i32,
    voice_sum_r_id: i32,
    pan_id: i32,
    filter_id: i32,
    delay_id: i32,
    send_id: i32,
    mod_out_id: i32,
    mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    mod_env_id: i32,
}

struct RackSlotMixer {
    slot_sum_l_id: i32,
    slot_sum_r_id: i32,
    slot_pan_id: i32,
}

struct SamplerVoiceSetup {
    sampler_ids: Vec<i32>,
    gatepitch_ids: Vec<i32>,
    modulator_ids: Vec<i32>,
    voice_lids: Vec<u64>,
}

enum InstrumentRegistration<'a> {
    Sampler {
        buffer_id: i32,
        sample_rate: u32,
        sampler_ids: Vec<i32>,
        gatepitch_ids: Vec<i32>,
        modulator_ids: Vec<i32>,
    },
    Custom {
        engine_id: usize,
        manifest: &'a DGenManifest,
        run_mode: CustomInstrumentRunMode,
    },
    Modulator,
}

struct TrackRegistration<'a> {
    idx: usize,
    track_name: String,
    shell: TrackShell,
    voice_lids: Vec<u64>,
    instrument: InstrumentRegistration<'a>,
}

mod types;
pub use types::{
    RackCustomBuildSpec, RackSamplerBuildSpec, RackSlotBuildSpec,
    RackSlotInstrumentBuildSpec,
};

pub struct GraphController<'a> {
    app: &'a mut App,
}

mod transaction;
use transaction::*;

fn push_graph_param(lg: *mut crate::audiograph::LiveGraph, logical_id: u64, idx: u64, value: f32) {
    if logical_id == 0 {
        return;
    }
    unsafe {
        crate::audiograph::params_push_wrapper(
            lg,
            crate::audiograph::ParamMsg {
                idx,
                logical_id,
                fvalue: value,
            },
        );
    }
}

fn push_graph_param_span(
    lg: *mut crate::audiograph::LiveGraph,
    logical_id: u64,
    idx: u64,
    span: u32,
    value: f32,
) {
    for lane in 0..span.max(1) as u64 {
        push_graph_param(lg, logical_id, idx + lane, value);
    }
}

fn normalized_host_input_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn host_signal_output_for_input(
    manifest: &DGenManifest,
    input: &lisp_host::DGenInput,
) -> Option<i32> {
    let manifest_lacks_names = manifest
        .inputs
        .iter()
        .all(|candidate| candidate.name.trim().is_empty());
    let normalized = normalized_host_input_name(&input.name);
    match normalized.as_str() {
        "gate" => Some(crate::effects::gatepitch::PARAM_GATE as i32),
        "pitch" => Some(crate::effects::gatepitch::PARAM_PITCH as i32),
        "velocity" | "vel" => Some(crate::effects::gatepitch::PARAM_VELOCITY as i32),
        "trigger" | "trig" => Some(crate::effects::gatepitch::PARAM_TRIGGER as i32),
        "clock" | "barclock" => Some(crate::effects::gatepitch::PARAM_CLOCK_PHASE as i32),
        _ if manifest_lacks_names && input.channel < 4 => Some(input.channel as i32),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum CustomHostInputSource {
    GatePitch(i32),
    Modulator(i32),
}

#[derive(Clone, Copy)]
struct CustomHostInputRoute {
    source: CustomHostInputSource,
    input_channel: i32,
}

fn custom_host_input_routes(
    manifest: &DGenManifest,
    context: &str,
) -> Result<Vec<CustomHostInputRoute>, String> {
    let mut routes = Vec::new();
    for input in &manifest.inputs {
        if input.channel >= manifest.n_inputs {
            return Err(format!(
                "{context}: manifest input '{}' references channel {} but instrument has {} inputs",
                input.name, input.channel, manifest.n_inputs
            ));
        }
        if manifest
            .modulators
            .iter()
            .any(|modulator| modulator.input_channel == input.channel)
        {
            continue;
        }
        if let Some(host_output) = host_signal_output_for_input(manifest, input) {
            routes.push(CustomHostInputRoute {
                source: CustomHostInputSource::GatePitch(host_output),
                input_channel: input.channel as i32,
            });
        }
    }

    for modulator in &manifest.modulators {
        if modulator.input_channel >= manifest.n_inputs {
            return Err(format!(
                "{context}: modulator '{}' references channel {} but instrument has {} inputs",
                modulator.name, modulator.input_channel, manifest.n_inputs
            ));
        }
        if modulator.slot == 0 || modulator.slot > crate::instruments::voice_modulator::NUM_OUTPUTS {
            return Err(format!(
                "{context}: modulator '{}' has invalid slot {}",
                modulator.name, modulator.slot
            ));
        }
        routes.push(CustomHostInputRoute {
            source: CustomHostInputSource::Modulator((modulator.slot - 1) as i32),
            input_channel: modulator.input_channel as i32,
        });
    }
    Ok(routes)
}

fn manifest_mod_output_channels(manifest: &DGenManifest) -> Vec<usize> {
    let output_count = manifest.n_outputs.max(1);
    let mut channels = Vec::new();
    for output in &manifest.mod_outputs {
        if output.channel < output_count && !channels.contains(&output.channel) {
            channels.push(output.channel);
        }
    }
    channels
}

fn manifest_audio_output_channels(manifest: &DGenManifest) -> Vec<usize> {
    let output_count = manifest.n_outputs.max(1);
    let mod_channels = manifest_mod_output_channels(manifest);
    (0..output_count)
        .filter(|channel| !mod_channels.contains(channel))
        .collect()
}

fn stereo_route_source_channel(audio_channels: &[usize], route_idx: usize) -> Option<usize> {
    match route_idx {
        0 => audio_channels.first().copied(),
        1 => audio_channels
            .get(1)
            .copied()
            .or_else(|| audio_channels.first().copied()),
        _ => None,
    }
}

fn engine_route_build_capacities(engine: &EngineNodeIds) -> (usize, usize) {
    let route_nodes_per_voice = 2 + if engine.modulator_ids.is_empty() {
        0
    } else {
        EXT_MOD_INPUT_COUNT
    };
    let connections_per_voice = 2
        + usize::from(stereo_route_source_channel(&engine.audio_output_channels, 0).is_some())
        + usize::from(stereo_route_source_channel(&engine.audio_output_channels, 1).is_some())
        + usize::from(!engine.mod_output_channels.is_empty())
        + if engine.modulator_ids.is_empty() {
            0
        } else {
            EXT_MOD_INPUT_COUNT * 2
        };
    (
        MAX_VOICES * route_nodes_per_voice,
        MAX_VOICES * connections_per_voice,
    )
}

fn sampler_voice_build_capacities(voice_count: usize) -> (usize, usize) {
    let voice_count = voice_count.clamp(1, MAX_VOICES);
    let nodes_per_voice = 3;
    let connections_per_voice =
        4 + 2 + crate::instruments::voice_modulator::NUM_OUTPUTS + EXT_MOD_INPUT_COUNT + 2;
    (
        voice_count * nodes_per_voice,
        voice_count * connections_per_voice,
    )
}

fn sampler_voice_delete_command_count(track: &TrackNodeIds) -> usize {
    track.sampler_ids.len() + track.sampler_gatepitch_ids.len() + track.sampler_modulator_ids.len()
}

fn positive_route_node_count(routes: &[[i32; 2]]) -> usize {
    routes
        .iter()
        .flat_map(|route_pair| route_pair.iter())
        .filter(|route_id| **route_id > 0)
        .count()
}

fn positive_ext_route_node_count(routes: &[[i32; EXT_MOD_INPUT_COUNT]]) -> usize {
    routes
        .iter()
        .flat_map(|route_ids| route_ids.iter())
        .filter(|route_id| **route_id > 0)
        .count()
}

fn engine_route_delete_command_count(engine: &EngineNodeIds, track: usize) -> usize {
    let route_nodes = engine
        .route_gain_ids
        .get(track)
        .map(|routes| positive_route_node_count(routes))
        .unwrap_or(0);
    let ext_route_nodes = engine
        .ext_route_gain_ids
        .get(track)
        .map(|routes| positive_ext_route_node_count(routes))
        .unwrap_or(0);
    let mod_output_disconnects = if engine.mod_output_channels.is_empty() {
        0
    } else {
        engine.synth_ids.len()
    };
    route_nodes + ext_route_nodes + mod_output_disconnects
}

fn engine_runtime_delete_command_count_excluding_track(
    engine: &EngineNodeIds,
    excluded_track: usize,
) -> usize {
    let route_nodes = engine
        .route_gain_ids
        .iter()
        .enumerate()
        .filter(|(track, _)| *track != excluded_track)
        .map(|(_, routes)| positive_route_node_count(routes))
        .sum::<usize>();
    let ext_route_nodes = engine
        .ext_route_gain_ids
        .iter()
        .enumerate()
        .filter(|(track, _)| *track != excluded_track)
        .map(|(_, routes)| positive_ext_route_node_count(routes))
        .sum::<usize>();
    route_nodes
        + ext_route_nodes
        + engine.synth_ids.len()
        + engine.modulator_ids.len()
        + engine.gatepitch_ids.len()
}

fn require_graph_edit_queue_capacity(
    lg: *mut crate::audiograph::LiveGraph,
    required: usize,
    context: &str,
) -> Result<(), String> {
    let available = unsafe { crate::audiograph::graph_edit_queue_available(lg) } as usize;
    if available < required {
        return Err(format!(
            "{context}: graph edit queue has room for {available} commands; {required} are required"
        ));
    }
    Ok(())
}

impl App {
    pub fn graph_controller(&mut self) -> GraphController<'_> {
        GraphController { app: self }
    }
}

mod bus_routing;
mod engine_connect;
mod engine_voice;
mod mod_routes;
mod node_build;
mod rack_rebuild;
mod rack_slots;
mod reorder;
mod slot_bookkeeping;
mod sync_clear;
mod teardown;
mod track_create;

fn delete_without_shift_enabled() -> bool {
    match std::env::var(DELETE_WITHOUT_SHIFT_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
