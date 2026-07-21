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
use crate::voice::MAX_VOICES;

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

#[derive(Clone)]
pub struct RackSamplerBuildSpec {
    pub buffer_id: i32,
    pub sample_rate: u32,
    pub sample_name: String,
}

pub struct RackCustomBuildSpec<'a> {
    pub instrument_name: &'a str,
    pub engine_id: usize,
    pub manifest: &'a DGenManifest,
    pub lib: &'a LoadedDGenLib,
    pub run_mode: CustomInstrumentRunMode,
}

pub enum RackSlotInstrumentBuildSpec<'a> {
    Sampler(RackSamplerBuildSpec),
    Custom(RackCustomBuildSpec<'a>),
}

pub struct RackSlotBuildSpec<'a> {
    pub instrument: RackSlotInstrumentBuildSpec<'a>,
    pub instrument_base_note_offset: f32,
    pub pad_note: Option<i32>,
    pub choke_group: Option<u8>,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub max_polyphony: usize,
    pub param_plocks: Option<RackSlotParamPlocks>,
    pub instrument_slot: Option<EffectSlotSnapshot>,
    pub effect_slots: Option<Vec<EffectSlotSnapshot>>,
    pub effect_descriptors: Option<Vec<EffectDescriptor>>,
    pub custom_effect_names: Option<Vec<Option<String>>>,
    pub track_sound_state: Option<TrackSoundState>,
}

pub struct GraphController<'a> {
    app: &'a mut App,
}

struct GraphEditBatchGuard {
    lg: *mut crate::audiograph::LiveGraph,
    serial: u64,
}

impl GraphEditBatchGuard {
    fn new(lg: *mut crate::audiograph::LiveGraph) -> Self {
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        let serial = unsafe { crate::audiograph::graph_edit_current_batch_serial(lg) };
        debug_assert!(serial > 0);
        Self { lg, serial }
    }
}

impl Drop for GraphEditBatchGuard {
    fn drop(&mut self) {
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
    }
}

unsafe fn disconnect_all_ports(lg: *mut crate::audiograph::LiveGraph, src_id: i32, dst_id: i32) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            crate::audiograph::graph_disconnect(lg, src_id, src_port, dst_id, dst_port);
        }
    }
}

unsafe fn connect_stereo_pair(lg: *mut crate::audiograph::LiveGraph, src_id: i32, dst_id: i32) {
    crate::audiograph::graph_connect(lg, src_id, 0, dst_id, 0);
    crate::audiograph::graph_connect(lg, src_id, 1, dst_id, 1);
}

fn add_gain_node_checked(
    lg: *mut crate::audiograph::LiveGraph,
    gain: f32,
    name: &str,
    context: &str,
) -> Result<i32, String> {
    let c_name = CString::new(name).map_err(|_| format!("{context}: node name contains NUL"))?;
    let node_id = unsafe { crate::audiograph::add_gain_node(lg, gain, c_name.as_ptr()) };
    if node_id < 0 {
        return Err(format!("{context}: failed to queue gain node '{name}'"));
    }
    Ok(node_id)
}

struct GraphNodeBuildTransaction {
    lg: *mut crate::audiograph::LiveGraph,
    node_ids: Vec<i32>,
    connections: Vec<(i32, i32, i32, i32)>,
    max_nodes: usize,
    max_connections: usize,
    finished: bool,
}

impl GraphNodeBuildTransaction {
    fn new(
        lg: *mut crate::audiograph::LiveGraph,
        max_nodes: usize,
        max_connections: usize,
    ) -> Result<Self, String> {
        let required_edits = max_nodes
            .checked_add(max_connections)
            .and_then(|forward_edits| forward_edits.checked_mul(2))
            .ok_or_else(|| "Graph edit transaction capacity overflow".to_string())?;
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        // GraphEditQueue is single-producer. Reserving room for both the
        // complete build and its inverse commands makes Drop rollback
        // infallible without rewinding a queue the audio thread may be reading.
        let available_edits = unsafe { crate::audiograph::graph_edit_queue_available(lg) } as usize;
        if available_edits < required_edits {
            unsafe { crate::audiograph::end_graph_edit_batch(lg) };
            return Err(format!(
                "Graph edit queue has room for {available_edits} commands; route construction requires {required_edits} for build and rollback"
            ));
        }
        Ok(Self {
            lg,
            node_ids: Vec::with_capacity(max_nodes),
            connections: Vec::with_capacity(max_connections),
            max_nodes,
            max_connections,
            finished: false,
        })
    }

    fn own(&mut self, node_id: i32) -> Result<i32, String> {
        self.node_ids.push(node_id);
        #[cfg(test)]
        record_test_graph_build_node(node_id);
        if self.node_ids.len() > self.max_nodes {
            return Err(format!(
                "Graph edit transaction created more than {} reserved nodes",
                self.max_nodes
            ));
        }
        Ok(node_id)
    }

    fn connect(
        &mut self,
        src_node: i32,
        src_port: i32,
        dst_node: i32,
        dst_port: i32,
        context: &str,
    ) -> Result<(), String> {
        if self.connections.len() >= self.max_connections {
            return Err(format!(
                "{context}: graph edit transaction exceeded its {} reserved connections",
                self.max_connections
            ));
        }
        let connected = unsafe {
            crate::audiograph::graph_connect(self.lg, src_node, src_port, dst_node, dst_port)
        };
        if !connected {
            return Err(format!(
                "{context}: graph_connect({src_node}, {src_port}, {dst_node}, {dst_port}) failed"
            ));
        }
        self.connections
            .push((src_node, src_port, dst_node, dst_port));
        #[cfg(test)]
        record_test_graph_build_connection((src_node, src_port, dst_node, dst_port));
        Ok(())
    }

    fn commit(mut self) {
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
        self.finished = true;
    }
}

impl Drop for GraphNodeBuildTransaction {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut rollback_succeeded = true;
        for &(src_node, src_port, dst_node, dst_port) in self.connections.iter().rev() {
            let queued = unsafe {
                crate::audiograph::graph_disconnect(self.lg, src_node, src_port, dst_node, dst_port)
            };
            rollback_succeeded &= queued;
            #[cfg(test)]
            if queued {
                record_test_graph_build_rollback_connection((
                    src_node, src_port, dst_node, dst_port,
                ));
            }
        }
        for &node_id in self.node_ids.iter().rev() {
            let queued = unsafe { crate::audiograph::delete_node(self.lg, node_id) };
            rollback_succeeded &= queued;
            #[cfg(test)]
            if queued {
                record_test_graph_build_rollback_node(node_id);
            }
        }
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
        self.finished = true;
        if !rollback_succeeded {
            eprintln!(
                "Graph edit rollback could not enqueue every inverse command despite its capacity reservation"
            );
        }
    }
}

fn add_engine_route_gain_node_checked(
    lg: *mut crate::audiograph::LiveGraph,
    gain: f32,
    name: &str,
    context: &str,
) -> Result<i32, String> {
    check_test_graph_build_node_add(context)?;
    add_gain_node_checked(lg, gain, name, context)
}

fn check_test_graph_build_node_add(context: &str) -> Result<(), String> {
    #[cfg(test)]
    if should_fail_test_graph_build_node_add() {
        return Err(format!("{context}: injected graph node allocation failure"));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_GRAPH_BUILD_FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static TEST_GRAPH_BUILD_NODE_IDS: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_GRAPH_BUILD_CONNECTIONS: std::cell::RefCell<Vec<(i32, i32, i32, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS: std::cell::RefCell<Vec<(i32, i32, i32, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn begin_test_graph_build_capture() {
    TEST_GRAPH_BUILD_NODE_IDS.with(|ids| ids.borrow_mut().clear());
    TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS.with(|ids| ids.borrow_mut().clear());
    TEST_GRAPH_BUILD_CONNECTIONS.with(|connections| connections.borrow_mut().clear());
    TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS.with(|connections| connections.borrow_mut().clear());
    TEST_GRAPH_BUILD_FAIL_AFTER.with(|remaining| remaining.set(None));
}

#[cfg(test)]
fn set_test_graph_build_failure_after(successful_adds: usize) {
    begin_test_graph_build_capture();
    TEST_GRAPH_BUILD_FAIL_AFTER.with(|remaining| remaining.set(Some(successful_adds)));
}

#[cfg(test)]
fn should_fail_test_graph_build_node_add() -> bool {
    TEST_GRAPH_BUILD_FAIL_AFTER.with(|remaining| match remaining.get() {
        Some(0) => {
            remaining.set(None);
            true
        }
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
        None => false,
    })
}

#[cfg(test)]
fn record_test_graph_build_node(node_id: i32) {
    TEST_GRAPH_BUILD_NODE_IDS.with(|ids| ids.borrow_mut().push(node_id));
}

#[cfg(test)]
fn record_test_graph_build_rollback_node(node_id: i32) {
    TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS.with(|ids| ids.borrow_mut().push(node_id));
}

#[cfg(test)]
fn record_test_graph_build_connection(connection: (i32, i32, i32, i32)) {
    TEST_GRAPH_BUILD_CONNECTIONS.with(|connections| connections.borrow_mut().push(connection));
}

#[cfg(test)]
fn record_test_graph_build_rollback_connection(connection: (i32, i32, i32, i32)) {
    TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS
        .with(|connections| connections.borrow_mut().push(connection));
}

#[cfg(test)]
fn take_test_graph_build_node_ids() -> Vec<i32> {
    TEST_GRAPH_BUILD_NODE_IDS.with(|ids| std::mem::take(&mut *ids.borrow_mut()))
}

#[cfg(test)]
fn take_test_graph_build_rollback_node_ids() -> Vec<i32> {
    TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS.with(|ids| std::mem::take(&mut *ids.borrow_mut()))
}

#[cfg(test)]
fn take_test_graph_build_connections() -> Vec<(i32, i32, i32, i32)> {
    TEST_GRAPH_BUILD_CONNECTIONS.with(|connections| std::mem::take(&mut *connections.borrow_mut()))
}

#[cfg(test)]
fn take_test_graph_build_rollback_connections() -> Vec<(i32, i32, i32, i32)> {
    TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS
        .with(|connections| std::mem::take(&mut *connections.borrow_mut()))
}

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
        if modulator.slot == 0 || modulator.slot > crate::voice_modulator::NUM_OUTPUTS {
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
        4 + 2 + crate::voice_modulator::NUM_OUTPUTS + EXT_MOD_INPUT_COUNT + 2;
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

impl GraphController<'_> {
    pub fn sync_current_pattern_mod_routes(&mut self) {
        let track_count = self.app.tracks.len();
        let connections = self.app.state.current_mod_connections();
        let mut desired: Vec<(i32, i32)> = Vec::with_capacity(connections.len());
        for connection in connections {
            if connection.source_track >= track_count
                || connection.dest_input >= EXT_MOD_INPUT_COUNT
                || !self
                    .app
                    .graph
                    .track_exposes_mod_output(connection.source_track)
            {
                continue;
            }
            if matches!(connection.destination, ModDestination::Track(dest) if dest == connection.source_track)
            {
                continue;
            }
            let source_id = self.app.graph.track_node_ids[connection.source_track].mod_out_id;
            let Some(dest_id) =
                self.resolve_mod_destination_input(connection.destination, connection.dest_input)
            else {
                continue;
            };
            if !desired.contains(&(source_id, dest_id)) {
                desired.push((source_id, dest_id));
            }
        }

        let applied = std::mem::take(&mut self.app.graph.applied_mod_routes);
        let mut changed = false;
        for (source_id, dest_id) in &applied {
            if !desired.contains(&(*source_id, *dest_id)) {
                changed = true;
                unsafe {
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        *source_id,
                        0,
                        *dest_id,
                        0,
                    );
                }
            }
        }
        for (source_id, dest_id) in &desired {
            if !applied.contains(&(*source_id, *dest_id)) {
                changed = true;
                unsafe {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        *source_id,
                        0,
                        *dest_id,
                        0,
                    );
                }
            }
        }
        self.app.graph.applied_mod_routes = desired;
        if changed {
            self.app.state.publish_scheduler_snapshot();
        }
    }

    fn resolve_mod_destination_input(
        &self,
        destination: ModDestination,
        input: usize,
    ) -> Option<i32> {
        if input >= EXT_MOD_INPUT_COUNT {
            return None;
        }
        match destination {
            ModDestination::Track(track) => self
                .app
                .graph
                .track_node_ids
                .get(track)
                .map(|nodes| nodes.mod_in_clip_ids[input]),
            ModDestination::Bus(bus_id) => self
                .app
                .graph
                .bus_node_ids
                .iter()
                .find(|bus| bus.id == bus_id)
                .map(|nodes| nodes.mod_in_clip_ids[input]),
        }
    }

    fn validate_mod_destination(
        &self,
        source_track: usize,
        destination: ModDestination,
        dest_input: usize,
    ) -> Result<(), String> {
        if dest_input >= EXT_MOD_INPUT_COUNT {
            return Err("mod route input out of range".to_string());
        }
        match destination {
            ModDestination::Track(dest_track) => {
                if dest_track >= self.app.tracks.len() {
                    return Err("mod route destination track out of range".to_string());
                }
                if source_track == dest_track {
                    return Err("mod route cannot connect a track to itself".to_string());
                }
            }
            ModDestination::Bus(bus_id) => {
                if !self
                    .app
                    .graph
                    .bus_node_ids
                    .iter()
                    .any(|bus| bus.id == bus_id)
                {
                    return Err("mod route destination bus not found".to_string());
                }
            }
        }
        Ok(())
    }

    pub fn set_mod_route_to_destination(
        &mut self,
        source_track: usize,
        destination: ModDestination,
        dest_input: usize,
    ) -> Result<(), String> {
        let track_count = self.app.tracks.len();
        if source_track >= track_count {
            return Err("mod route source track out of range".to_string());
        }
        if !self.app.graph.track_exposes_mod_output(source_track) {
            return Err("mod route source track has no mod output".to_string());
        }
        self.validate_mod_destination(source_track, destination, dest_input)?;
        self.app.state.edit_current_mod_connections(|connections| {
            let connection = crate::sequencer::ModConnection {
                source_track,
                destination,
                dest_input,
            };
            if !connections.contains(&connection) {
                connections.push(connection);
            }
            Ok(())
        })?;
        self.sync_current_pattern_mod_routes();
        Ok(())
    }

    pub fn set_mod_route(
        &mut self,
        source_track: usize,
        dest_track: usize,
        dest_input: usize,
    ) -> Result<(), String> {
        self.set_mod_route_to_destination(
            source_track,
            ModDestination::Track(dest_track),
            dest_input,
        )
    }

    pub fn delete_mod_route_to_destination(
        &mut self,
        source_track: usize,
        destination: ModDestination,
        dest_input: usize,
    ) -> Result<(), String> {
        self.app.state.edit_current_mod_connections(|connections| {
            connections.retain(|connection| {
                connection.source_track != source_track
                    || connection.destination != destination
                    || connection.dest_input != dest_input
            });
            Ok(())
        })?;
        self.sync_current_pattern_mod_routes();
        Ok(())
    }

    pub fn delete_mod_route(
        &mut self,
        source_track: usize,
        dest_track: usize,
        dest_input: usize,
    ) -> Result<(), String> {
        self.delete_mod_route_to_destination(
            source_track,
            ModDestination::Track(dest_track),
            dest_input,
        )
    }

    pub fn ensure_bus_graph_node(&mut self, id: BusId, name: &str) {
        if id == BusId::MIX || self.app.graph.bus_node_ids.iter().any(|bus| bus.id == id) {
            return;
        }

        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let safe_name: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let left_name = format!("{safe_name}_L");
        let right_name = format!("{safe_name}_R");
        let merge_name = CString::new(format!("{safe_name}_merge")).unwrap();
        let gate_name = CString::new(format!("{safe_name}_gate")).unwrap();
        let volume_name = CString::new(format!("{safe_name}_volume")).unwrap();
        let mod_in_clip_ids = std::array::from_fn(|input| {
            let mod_in_name =
                CString::new(format!("{safe_name}_mod_in{}_clip", input + 1)).unwrap();
            unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::track_modulator::mod_in_clip_vtable(),
                    crate::track_modulator::MOD_IN_CLIP_STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_in_name.as_ptr(),
                    1,
                    1,
                    std::ptr::null(),
                    0,
                )
            }
        });
        let left_id = match add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &left_name,
            "ensure_bus_graph_node left bus input",
        ) {
            Ok(node_id) => node_id,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };
        let right_id = match add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &right_name,
            "ensure_bus_graph_node right bus input",
        ) {
            Ok(node_id) => node_id,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };
        let merge_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                merge_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let volume_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                volume_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let gate_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                gate_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, merge_id);
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, gate_id);
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, volume_id);
        }
        unsafe {
            crate::audiograph::graph_connect(self.app.graph.lg.0, left_id, 0, merge_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, right_id, 0, merge_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, merge_id, 0, gate_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, merge_id, 1, gate_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, gate_id, 0, volume_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, gate_id, 1, volume_id, 1);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                volume_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                volume_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
        }
        self.app.graph.bus_node_ids.push(super::BusNodeIds {
            id,
            left_id,
            right_id,
            merge_id,
            gate_id,
            volume_id,
            mod_in_clip_ids,
        });
        self.app.publish_bus_gate_runtime();
    }

    /// Makes the graph bus registry exactly mirror the current project bus
    /// registry, including ordering. Several realtime/UI bridges intentionally
    /// use compact bus indices, so this invariant must be restored whenever a
    /// project replaces `App::buses` while retaining the live graph.
    pub fn reconcile_bus_graph_nodes(&mut self) -> Result<(), String> {
        let project_buses = self
            .app
            .buses
            .iter()
            .map(|bus| (bus.id, bus.name.clone()))
            .collect::<Vec<_>>();
        let stale_ids = self
            .app
            .graph
            .bus_node_ids
            .iter()
            .map(|nodes| nodes.id)
            .filter(|id| !project_buses.iter().any(|(project_id, _)| project_id == id))
            .collect::<Vec<_>>();
        for id in stale_ids {
            self.delete_bus_graph_node(id);
        }
        for (id, name) in &project_buses {
            self.ensure_bus_graph_node(*id, name);
        }

        for (id, name) in &project_buses {
            if !self
                .app
                .graph
                .bus_node_ids
                .iter()
                .any(|nodes| nodes.id == *id)
            {
                return Err(format!(
                    "Graph nodes for bus '{name}' ({}) were not created",
                    id.0
                ));
            }
        }
        self.app.graph.bus_node_ids.sort_by_key(|nodes| {
            project_buses
                .iter()
                .position(|(id, _)| *id == nodes.id)
                .expect("graph bus membership was validated before sorting")
        });
        self.app.publish_bus_gate_runtime();
        Ok(())
    }

    pub fn delete_bus_graph_node(&mut self, id: BusId) {
        let Some(pos) = self
            .app
            .graph
            .bus_node_ids
            .iter()
            .position(|bus| bus.id == id)
        else {
            return;
        };
        let bus = self.app.graph.bus_node_ids.remove(pos);
        unsafe {
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, bus.merge_id);
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, bus.gate_id);
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, bus.volume_id);
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.volume_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.volume_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.left_id,
                0,
                bus.merge_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.right_id,
                0,
                bus.merge_id,
                1,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.merge_id,
                0,
                bus.gate_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.merge_id,
                1,
                bus.gate_id,
                1,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.gate_id,
                0,
                bus.volume_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.gate_id,
                1,
                bus.volume_id,
                1,
            );
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.merge_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.gate_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.volume_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.left_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.right_id);
            for &mod_in_clip_id in &bus.mod_in_clip_ids {
                crate::audiograph::delete_node(self.app.graph.lg.0, mod_in_clip_id);
            }
        }
        self.app.publish_bus_gate_runtime();
    }

    fn disconnect_delay_output_from_all(&self, delay_id: i32) {
        unsafe {
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                delay_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                delay_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
            for bus in &self.app.graph.bus_node_ids {
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    delay_id,
                    0,
                    bus.left_id,
                    0,
                );
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    delay_id,
                    1,
                    bus.right_id,
                    0,
                );
            }
        }
    }

    fn connect_delay_output_to(&self, delay_id: i32, output: &TrackOutput) {
        unsafe {
            match output {
                TrackOutput::Mix => {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        delay_id,
                        0,
                        self.app.graph.bus_l_id,
                        0,
                    );
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        delay_id,
                        1,
                        self.app.graph.bus_r_id,
                        0,
                    );
                }
                TrackOutput::Bus(id) => {
                    if let Some(bus) = self.app.graph.bus_node_ids.iter().find(|bus| bus.id == *id)
                    {
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            0,
                            bus.left_id,
                            0,
                        );
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            1,
                            bus.right_id,
                            0,
                        );
                    } else {
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            0,
                            self.app.graph.bus_l_id,
                            0,
                        );
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            1,
                            self.app.graph.bus_r_id,
                            0,
                        );
                    }
                }
                TrackOutput::None => {}
            }
        }
    }

    pub fn apply_track_output_routing(&mut self, track_idx: usize) {
        let Some(nodes) = self.app.graph.track_node_ids.get(track_idx) else {
            return;
        };
        let output = self.app.state.pattern.track_params[track_idx].output();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.disconnect_delay_output_from_all(nodes.delay_id);
        self.connect_delay_output_to(nodes.delay_id, &output);
    }

    pub fn apply_track_bus_sends(&mut self, track_idx: usize) {
        let Some(nodes) = self.app.graph.track_node_ids.get_mut(track_idx) else {
            return;
        };
        let delay_id = nodes.delay_id;
        let mut old_sends = std::mem::take(&mut nodes.bus_send_ids);
        let requested_sends = self.app.state.pattern.track_params[track_idx].sends();
        let bus_nodes = self.app.graph.bus_node_ids.clone();
        let lg = self.app.graph.lg.0;

        let _batch = GraphEditBatchGuard::new(lg);
        let mut next_send_nodes = Vec::new();

        for send in requested_sends {
            if send.amount <= 0.0 {
                continue;
            }
            let Some(bus) = bus_nodes.iter().find(|bus| bus.id == send.destination) else {
                continue;
            };

            if let Some(pos) = old_sends
                .iter()
                .position(|nodes| nodes.destination == send.destination)
            {
                let existing = old_sends.remove(pos);
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        lg,
                        crate::audiograph::ParamMsg {
                            idx: 0,
                            logical_id: existing.left_id as u64,
                            fvalue: send.amount,
                        },
                    );
                    crate::audiograph::params_push_wrapper(
                        lg,
                        crate::audiograph::ParamMsg {
                            idx: 0,
                            logical_id: existing.right_id as u64,
                            fvalue: send.amount,
                        },
                    );
                }
                next_send_nodes.push(existing);
                continue;
            }

            let left_name = format!("track_{track_idx}_send_{}_L", send.destination.0);
            let right_name = format!("track_{track_idx}_send_{}_R", send.destination.0);
            let left_id = match add_gain_node_checked(
                lg,
                send.amount,
                &left_name,
                "apply_track_bus_sends left send",
            ) {
                Ok(node_id) => node_id,
                Err(error) => {
                    eprintln!("{error}");
                    continue;
                }
            };
            let right_id = match add_gain_node_checked(
                lg,
                send.amount,
                &right_name,
                "apply_track_bus_sends right send",
            ) {
                Ok(node_id) => node_id,
                Err(error) => {
                    eprintln!("{error}");
                    unsafe {
                        crate::audiograph::delete_node(lg, left_id);
                    }
                    continue;
                }
            };
            unsafe {
                crate::audiograph::graph_connect(lg, delay_id, 0, left_id, 0);
                crate::audiograph::graph_connect(lg, delay_id, 1, right_id, 0);
                crate::audiograph::graph_connect(lg, left_id, 0, bus.left_id, 0);
                crate::audiograph::graph_connect(lg, right_id, 0, bus.right_id, 0);
            }
            next_send_nodes.push(super::BusSendNodeIds {
                destination: send.destination,
                left_id,
                right_id,
            });
        }

        for send in old_sends {
            if let Some(bus) = bus_nodes.iter().find(|bus| bus.id == send.destination) {
                unsafe {
                    crate::audiograph::graph_disconnect(lg, delay_id, 0, send.left_id, 0);
                    crate::audiograph::graph_disconnect(lg, delay_id, 1, send.right_id, 0);
                    crate::audiograph::graph_disconnect(lg, send.left_id, 0, bus.left_id, 0);
                    crate::audiograph::graph_disconnect(lg, send.right_id, 0, bus.right_id, 0);
                }
            }
            unsafe {
                crate::audiograph::delete_node(lg, send.left_id);
                crate::audiograph::delete_node(lg, send.right_id);
            }
        }

        let Some(nodes) = self.app.graph.track_node_ids.get_mut(track_idx) else {
            return;
        };
        nodes.bus_send_ids = next_send_nodes;
    }

    pub fn add_track(&mut self, wav_path: &Path) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let loaded = crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let buffer_id = loaded.buffer_id;
        let sample_rate = loaded.sample_rate;
        let track_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        let shell = self.create_track_shell(idx, &track_name)?;
        let voices = self.build_sampler_voices(
            idx,
            &track_name,
            buffer_id,
            sample_rate,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
            shell.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Sampler {
                buffer_id,
                sample_rate,
                sampler_ids: voices.sampler_ids,
                gatepitch_ids: voices.gatepitch_ids,
                modulator_ids: voices.modulator_ids,
            },
        })?;
        let sample_path = wav_path.to_path_buf();
        let sample_name = self.app.tracks[idx].clone();
        self.app.sampler_paths.push(Some(sample_path.clone()));
        self.app
            .register_loaded_sample_path(&sample_name, buffer_id, sample_path);
        self.app.reset_sampler_bpm_for_analysis(idx);
        self.app.publish_sampler_analysis_runtime(idx);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_blank_sampler_track(&mut self) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let buffer_id = crate::sampler::create_silent_buffer(self.app.graph.lg.0)?;
        let sample_rate = self.app.graph.sample_rate;
        let track_name = format!("Sampler {}", idx + 1);
        let shell = self.create_track_shell(idx, &track_name)?;
        let voices = self.build_sampler_voices(
            idx,
            &track_name,
            buffer_id,
            sample_rate,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
            shell.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Sampler {
                buffer_id,
                sample_rate,
                sampler_ids: voices.sampler_ids,
                gatepitch_ids: voices.gatepitch_ids,
                modulator_ids: voices.modulator_ids,
            },
        })?;
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_modulator_track(&mut self) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let track_name = format!("Modulator {}", idx + 1);
        let shell = self.create_track_shell(idx, &track_name)?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: Vec::new(),
            instrument: InstrumentRegistration::Modulator,
        })?;
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_custom_track(
        &mut self,
        name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let track_name = instrument_display_name(name);
        let shell = self.create_track_shell(idx, &track_name)?;
        self.ensure_custom_engine_runtime(engine_id, name, manifest, lib)?;
        self.connect_engine_to_track(
            engine_id,
            idx,
            idx,
            &track_name,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
            shell.mod_out_id,
            shell.mod_in_clip_ids,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: Vec::new(),
            instrument: InstrumentRegistration::Custom {
                engine_id,
                manifest,
                run_mode,
            },
        })?;
        self.app.sampler_paths.push(None);
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(idx)?;
        }
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn swap_custom_track_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err(format!(
                "Track {} is not a custom instrument track",
                track + 1
            ));
        }
        let old_engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        for (len, collection) in [
            (
                self.app.graph.track_instrument_run_modes.len(),
                "instrument run modes",
            ),
            (
                self.app.graph.track_synth_node_ids.len(),
                "track synth node ids",
            ),
            (
                self.app.graph.track_gatepitch_node_ids.len(),
                "track gatepitch node ids",
            ),
            (
                self.app.graph.instrument_descriptors.len(),
                "instrument descriptors",
            ),
        ] {
            if track >= len {
                return Err(format!(
                    "Track {} is missing from {collection} (length {len})",
                    track + 1
                ));
            }
        }
        self.app
            .state
            .validate_instrument_slot_reset_target(track, new_engine_id)?;
        if new_engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {new_engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }

        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let track_name = self.app.tracks[track].clone();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let (new_synth_ids, new_gatepitch_ids, node_id, modulator_node_id) = {
            self.ensure_custom_engine_runtime(new_engine_id, instrument_name, manifest, lib)?;

            let new_engine = self.app.graph.engine_node_ids[new_engine_id]
                .as_ref()
                .ok_or_else(|| format!("Missing runtime for instrument engine {new_engine_id}"))?;
            let existing_routes = new_engine.route_gain_ids.get(track).ok_or_else(|| {
                format!(
                    "Track {} is outside engine {new_engine_id}'s route table",
                    track + 1
                )
            })?;
            let existing_ext_routes =
                new_engine.ext_route_gain_ids.get(track).ok_or_else(|| {
                    format!(
                        "Track {} is outside engine {new_engine_id}'s external route table",
                        track + 1
                    )
                })?;
            if new_engine_id != old_engine_id
                && (!existing_routes.is_empty() || !existing_ext_routes.is_empty())
            {
                return Err(format!(
                    "Instrument engine {new_engine_id} already has a route for track {}",
                    track + 1
                ));
            }

            let old_engine = self
                .app
                .graph
                .engine_node_ids
                .get(old_engine_id)
                .and_then(|engine| engine.as_ref())
                .ok_or_else(|| format!("Missing runtime for instrument engine {old_engine_id}"))?;
            if old_engine
                .route_gain_ids
                .get(track)
                .is_none_or(|routes| routes.len() != MAX_VOICES)
            {
                return Err(format!(
                    "Instrument engine {old_engine_id} does not have a complete route for track {}",
                    track + 1
                ));
            }

            let should_delete_old_runtime = new_engine_id != old_engine_id
                && !self.engine_is_still_referenced_excluding(old_engine_id, track);
            if new_engine_id != old_engine_id {
                let (route_nodes, route_connections) = engine_route_build_capacities(new_engine);
                let route_transaction_commands =
                    (route_nodes + route_connections)
                        .checked_mul(2)
                        .ok_or_else(|| "Instrument route capacity overflow".to_string())?;
                let old_route_delete_commands =
                    engine_route_delete_command_count(old_engine, track);
                let old_runtime_delete_commands = if should_delete_old_runtime {
                    engine_runtime_delete_command_count_excluding_track(old_engine, track)
                } else {
                    0
                };
                let required_commands = route_transaction_commands
                    .checked_add(old_route_delete_commands)
                    .and_then(|count| count.checked_add(old_runtime_delete_commands))
                    .ok_or_else(|| "Instrument swap graph capacity overflow".to_string())?;
                require_graph_edit_queue_capacity(
                    self.app.graph.lg.0,
                    required_commands,
                    "Instrument swap",
                )?;
            }

            if run_mode == CustomInstrumentRunMode::FreePatch
                && (new_engine.synth_ids.is_empty() || new_engine.gatepitch_ids.is_empty())
            {
                return Err(format!(
                    "Instrument engine {new_engine_id} has no voice 0 runtime for free-patch mode"
                ));
            }
            let new_synth_ids = new_engine.synth_ids.clone();
            let new_gatepitch_ids = new_engine.gatepitch_ids.clone();
            let node_id = first_graph_node_identity(&new_engine.synth_ids);
            let modulator_node_id = first_graph_node_identity(&new_engine.modulator_ids);

            self.connect_engine_to_track(
                new_engine_id,
                track,
                track,
                &track_name,
                track_nodes.voice_sum_id,
                track_nodes.voice_sum_r_id,
                track_nodes.mod_out_id,
                track_nodes.mod_in_clip_ids,
            )?;

            let new_engine = self.app.graph.engine_node_ids[new_engine_id]
                .as_ref()
                .expect("new engine runtime was validated above");
            debug_assert_eq!(
                new_engine.route_gain_ids[track].len(),
                MAX_VOICES,
                "successful route construction must publish every voice"
            );

            if new_engine_id != old_engine_id {
                self.delete_engine_route_for_track(old_engine_id, track, track);
                if should_delete_old_runtime {
                    self.delete_engine_runtime(old_engine_id);
                }
            }
            (new_synth_ids, new_gatepitch_ids, node_id, modulator_node_id)
        };

        self.app.graph.track_engine_ids[track] = Some(new_engine_id);
        self.app.graph.track_synth_node_ids[track] = new_synth_ids;
        self.app.graph.track_gatepitch_node_ids[track] = new_gatepitch_ids;
        self.app.graph.track_instrument_run_modes[track] = run_mode;
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self
            .app
            .state
            .reset_instrument_slot_all_patterns(
                track,
                &descriptor,
                node_id,
                modulator_node_id,
                new_engine_id,
                run_mode,
            )
            .expect("instrument reset target was validated before graph mutation");

        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)
                .expect("free-patch engine runtime was validated before graph mutation");
        }
        self.app.tracks[track] = instrument_display_name(instrument_name);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub fn replace_track_with_custom_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        match self.app.graph.track_instrument_types.get(track).copied() {
            Some(InstrumentType::Custom) => self.swap_custom_track_instrument(
                track,
                instrument_name,
                new_engine_id,
                manifest,
                lib,
                run_mode,
            ),
            Some(InstrumentType::Sampler) => self.convert_sampler_track_to_custom_instrument(
                track,
                instrument_name,
                new_engine_id,
                manifest,
                lib,
                run_mode,
            ),
            Some(InstrumentType::Rack) => self.convert_rack_track_to_custom_instrument(
                track,
                instrument_name,
                new_engine_id,
                manifest,
                lib,
                run_mode,
            ),
            Some(other) => Err(format!(
                "Track {} has instrument type {other:?}, which cannot be replaced with a custom instrument",
                track + 1
            )),
            None => Err(format!("Invalid track index {}", track + 1)),
        }
    }

    fn convert_sampler_track_to_custom_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Sampler) {
            return Err(format!("Track {} is not a sampler track", track + 1));
        }
        if self.app.graph.track_engine_ids.get(track) != Some(&None) {
            return Err(format!(
                "Sampler track {} has an unexpected custom engine binding",
                track + 1
            ));
        }
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        if track_nodes.sampler_ids.is_empty()
            || track_nodes.sampler_gatepitch_ids.len() != track_nodes.sampler_ids.len()
            || track_nodes.sampler_modulator_ids.len() != track_nodes.sampler_ids.len()
        {
            return Err(format!(
                "Sampler track {} does not have a complete voice pool",
                track + 1
            ));
        }
        self.app
            .state
            .validate_instrument_slot_reset_target(track, new_engine_id)?;
        if new_engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {new_engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }

        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let old_track_name = self.app.tracks[track].clone();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.ensure_custom_engine_runtime(new_engine_id, instrument_name, manifest, lib)?;
        let new_engine = self.app.graph.engine_node_ids[new_engine_id]
            .as_ref()
            .ok_or_else(|| format!("Missing runtime for instrument engine {new_engine_id}"))?;
        let existing_routes = new_engine.route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s route table",
                track + 1
            )
        })?;
        let existing_ext_routes = new_engine.ext_route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s external route table",
                track + 1
            )
        })?;
        if !existing_routes.is_empty() || !existing_ext_routes.is_empty() {
            return Err(format!(
                "Instrument engine {new_engine_id} already has a route for track {}",
                track + 1
            ));
        }
        if run_mode == CustomInstrumentRunMode::FreePatch
            && (new_engine.synth_ids.is_empty() || new_engine.gatepitch_ids.is_empty())
        {
            return Err(format!(
                "Instrument engine {new_engine_id} has no voice 0 runtime for free-patch mode"
            ));
        }
        let (route_nodes, route_connections) = engine_route_build_capacities(new_engine);
        let route_transaction_commands = (route_nodes + route_connections)
            .checked_mul(2)
            .ok_or_else(|| "Instrument route capacity overflow".to_string())?;
        let required_commands = route_transaction_commands
            .checked_add(sampler_voice_delete_command_count(&track_nodes))
            .ok_or_else(|| "Sampler conversion graph capacity overflow".to_string())?;
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Sampler-to-instrument conversion",
        )?;

        let new_synth_ids = new_engine.synth_ids.clone();
        let new_gatepitch_ids = new_engine.gatepitch_ids.clone();
        let node_id = first_graph_node_identity(&new_engine.synth_ids);
        let modulator_node_id = first_graph_node_identity(&new_engine.modulator_ids);
        self.connect_engine_to_track(
            new_engine_id,
            track,
            track,
            &old_track_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;
        self.delete_sampler_voice_nodes(&track_nodes);
        self.clear_sampler_runtime_pool(track);

        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.sampler_ids.clear();
        live_nodes.sampler_gatepitch_ids.clear();
        live_nodes.sampler_modulator_ids.clear();
        self.app.graph.track_voice_lids[track].clear();
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.track_instrument_types[track] = InstrumentType::Custom;
        self.app.graph.track_instrument_run_modes[track] = run_mode;
        self.app.graph.track_engine_ids[track] = Some(new_engine_id);
        self.app.graph.track_synth_node_ids[track] = new_synth_ids;
        self.app.graph.track_gatepitch_node_ids[track] = new_gatepitch_ids;
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self
            .app
            .state
            .reset_instrument_slot_all_patterns(
                track,
                &descriptor,
                node_id,
                modulator_node_id,
                new_engine_id,
                run_mode,
            )
            .expect("instrument reset target was validated before graph mutation");
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)
                .expect("free-patch engine runtime was validated before graph mutation");
        }
        self.app.tracks[track] = instrument_display_name(instrument_name);
        self.app.publish_sampler_analysis_runtime(track);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    fn convert_rack_track_to_custom_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err(format!("Track {} is not an instrument rack", track + 1));
        }
        if self.app.graph.track_engine_ids.get(track) != Some(&None) {
            return Err(format!(
                "Rack track {} has an unexpected flat engine binding",
                track + 1
            ));
        }
        let track_id = self.app.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let track_nodes = self.app.graph.track_node_ids.get(track).cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        self.app.state.validate_instrument_slot_reset_target(track, new_engine_id)?;
        if new_engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {new_engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }

        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let old_track_name = self.app.tracks[track].clone();
        let old_engine_ids = self.rack_engine_ids_for_track(track);
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.ensure_custom_engine_runtime(new_engine_id, instrument_name, manifest, lib)?;
        let new_engine = self.app.graph.engine_node_ids[new_engine_id]
            .as_ref()
            .ok_or_else(|| format!("Missing runtime for instrument engine {new_engine_id}"))?;
        let existing_routes = new_engine.route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s route table",
                track + 1
            )
        })?;
        let existing_ext_routes = new_engine.ext_route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s external route table",
                track + 1
            )
        })?;
        if !existing_routes.is_empty() || !existing_ext_routes.is_empty() {
            return Err(format!(
                "Instrument engine {new_engine_id} already has a route for track {}",
                track + 1
            ));
        }
        if run_mode == CustomInstrumentRunMode::FreePatch
            && (new_engine.synth_ids.is_empty() || new_engine.gatepitch_ids.is_empty())
        {
            return Err(format!(
                "Instrument engine {new_engine_id} has no voice 0 runtime for free-patch mode"
            ));
        }
        let (route_nodes, route_connections) = engine_route_build_capacities(new_engine);
        let required_commands = (route_nodes + route_connections)
            .checked_mul(2)
            .ok_or_else(|| "Instrument route capacity overflow".to_string())?;
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Rack-to-instrument conversion",
        )?;

        let new_synth_ids = new_engine.synth_ids.clone();
        let new_gatepitch_ids = new_engine.gatepitch_ids.clone();
        let node_id = first_graph_node_identity(&new_engine.synth_ids);
        let modulator_node_id = first_graph_node_identity(&new_engine.modulator_ids);
        self.connect_engine_to_track(
            new_engine_id,
            track,
            track,
            &old_track_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;
        self.delete_rack_effect_chains(track, batch.serial)?;
        self.retire_rack_slot_graph_generation(track);

        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.rack_slots.clear();
        live_nodes.rack_signature = None;
        self.app.graph.track_instrument_types[track] = InstrumentType::Custom;
        self.app.graph.track_instrument_run_modes[track] = run_mode;
        self.app.graph.track_engine_ids[track] = Some(new_engine_id);
        self.app.graph.track_synth_node_ids[track] = new_synth_ids;
        self.app.graph.track_gatepitch_node_ids[track] = new_gatepitch_ids;
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self.app.state.reset_instrument_slot_all_patterns(
            track,
            &descriptor,
            node_id,
            modulator_node_id,
            new_engine_id,
            run_mode,
        ).expect("instrument reset target was validated before graph mutation");
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)
                .expect("free-patch engine runtime was validated before graph mutation");
        }
        drop(batch);

        self.reap_excess_rack_teardowns();
        for engine_id in old_engine_ids {
            if engine_id != new_engine_id && !self.engine_is_still_referenced(engine_id) {
                lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
            }
        }
        self.app.tracks[track] = instrument_display_name(instrument_name);
        self.app.device_registry.clear_rack_track(track_id);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub fn convert_custom_track_to_sampler(
        &mut self,
        track: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err(format!(
                "Track {} is not a custom instrument track",
                track + 1
            ));
        }
        let old_engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        self.app.state.validate_sampler_slot_reset_target(track)?;
        let old_engine = self
            .app
            .graph
            .engine_node_ids
            .get(old_engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {old_engine_id}"))?;
        if old_engine
            .route_gain_ids
            .get(track)
            .is_none_or(|routes| routes.len() != MAX_VOICES)
        {
            return Err(format!(
                "Instrument engine {old_engine_id} does not have a complete route for track {}",
                track + 1
            ));
        }
        let should_delete_old_runtime =
            !self.engine_is_still_referenced_excluding(old_engine_id, track);
        let (sampler_nodes, sampler_connections) = sampler_voice_build_capacities(MAX_VOICES);
        let sampler_transaction_commands = (sampler_nodes + sampler_connections)
            .checked_mul(2)
            .ok_or_else(|| "Sampler voice capacity overflow".to_string())?;
        let old_route_delete_commands = engine_route_delete_command_count(old_engine, track);
        let old_runtime_delete_commands = if should_delete_old_runtime {
            engine_runtime_delete_command_count_excluding_track(old_engine, track)
        } else {
            0
        };
        let required_commands = sampler_transaction_commands
            .checked_add(old_route_delete_commands)
            .and_then(|count| count.checked_add(old_runtime_delete_commands))
            .ok_or_else(|| "Instrument-to-sampler graph capacity overflow".to_string())?;

        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Instrument-to-sampler conversion",
        )?;
        let voices = self.build_sampler_voices(
            track,
            sample_name,
            buffer_id,
            sample_rate,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.delete_engine_route_for_track(old_engine_id, track, track);
        if should_delete_old_runtime {
            self.delete_engine_runtime(old_engine_id);
        }

        let descriptor = EffectDescriptor::builtin_sampler();
        let node_id = first_graph_node_identity(&voices.sampler_ids);
        let modulator_node_id = first_graph_node_identity(&voices.modulator_ids);
        self.publish_sampler_voice_runtime(
            track,
            &voices.voice_lids,
            &voices.sampler_ids,
            &voices.gatepitch_ids,
            &voices.modulator_ids,
        );
        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.sampler_ids = voices.sampler_ids;
        live_nodes.sampler_gatepitch_ids = voices.gatepitch_ids;
        live_nodes.sampler_modulator_ids = voices.modulator_ids;
        self.app.graph.track_voice_lids[track] = voices.voice_lids;
        self.app.graph.track_buffer_ids[track] = buffer_id;
        self.app.graph.track_sample_rates[track] = sample_rate;
        self.app.graph.track_instrument_types[track] = InstrumentType::Sampler;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self
            .app
            .state
            .reset_sampler_slot_all_patterns(
                track,
                &descriptor,
                node_id,
                modulator_node_id,
                (buffer_id, sample_name.to_string(), sample_rate),
            )
            .expect("sampler reset target was validated before graph mutation");
        self.app.tracks[track] = sample_name.to_string();
        self.app.reset_sampler_bpm_for_analysis(track);
        self.app.publish_sampler_analysis_runtime(track);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    fn delete_sampler_voice_nodes(&self, track: &TrackNodeIds) {
        for node_id in track
            .sampler_ids
            .iter()
            .chain(&track.sampler_gatepitch_ids)
            .chain(&track.sampler_modulator_ids)
        {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, *node_id);
            }
        }
    }

    fn finish_track_instrument_source_change(&mut self, track: usize) {
        self.app.sync_scratch_runtime_descriptors();
        self.app
            .macro_engine
            .remove_instrument_mappings_for_track(track);
        self.app.push_instrument_defaults_for_track(track);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        // Publish only after both epochs advance. The scheduler stamps queued
        // events with snapshot.pattern_epoch while the audio callback rejects
        // events against the live atomic epoch; publishing the old epoch here
        // silences the swapped track until another transport action republishes.
        self.app
            .state
            .publish_macro_overrides(self.app.macro_engine.override_snapshot());
    }

    pub fn add_rack_track(
        &mut self,
        name: &str,
        routing: RackRouting,
        slots: Vec<RackSlotBuildSpec<'_>>,
    ) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        if slots.len() > MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }
        validate_rack_build_slot_pad_map(routing, &slots)?;
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let track_name = instrument_display_name(name);
        let shell = self.create_track_shell(idx, &track_name)?;
        let has_solo = slots.iter().any(|slot| slot.solo);
        let mut rack_slot_nodes = Vec::with_capacity(slots.len());
        let mut rack_slot_snapshots = Vec::with_capacity(slots.len());

        for (slot_idx, slot) in slots.into_iter().enumerate() {
            let slot_name = format!("{}_rack{}", track_name, slot_idx + 1);
            let mixer = self.create_rack_slot_mixer(
                &slot_name,
                shell.voice_sum_id,
                shell.voice_sum_r_id,
                slot.gain,
                slot.pan,
                slot.mute,
                has_solo && !slot.solo,
            )?;
            let max_polyphony = slot.max_polyphony.clamp(1, MAX_VOICES);
            match slot.instrument {
                RackSlotInstrumentBuildSpec::Sampler(sampler) => {
                    let Some(pool_id) = rack_slot_pool_index(idx, slot_idx) else {
                        return Err(format!(
                            "Rack sampler pool unavailable for track {idx} slot {slot_idx}"
                        ));
                    };
                    let voices = self.build_sampler_voices(
                        pool_id,
                        &slot_name,
                        sampler.buffer_id,
                        sampler.sample_rate,
                        mixer.slot_sum_l_id,
                        mixer.slot_sum_r_id,
                        shell.mod_in_clip_ids,
                        max_polyphony,
                    )?;
                    self.publish_sampler_voice_runtime(
                        pool_id,
                        &voices.voice_lids,
                        &voices.sampler_ids,
                        &voices.gatepitch_ids,
                        &voices.modulator_ids,
                    );
                    let mut instrument_slot = slot.instrument_slot.unwrap_or_else(|| {
                        EffectSlotSnapshot::new_default_with_modulator(
                            &EffectDescriptor::builtin_sampler(),
                            first_graph_node_identity(&voices.sampler_ids),
                            first_graph_node_identity(&voices.modulator_ids),
                        )
                    });
                    instrument_slot.sync_to_descriptor_with_modulator(
                        &EffectDescriptor::builtin_sampler(),
                        first_graph_node_identity(&voices.sampler_ids),
                        first_graph_node_identity(&voices.modulator_ids),
                    );
                    let sample_id = Some((
                        sampler.buffer_id,
                        sampler.sample_name.clone(),
                        sampler.sample_rate,
                    ));
                    rack_slot_snapshots.push(RackSlotSnapshot {
                        instrument_type: InstrumentType::Sampler,
                        instrument_run_mode: CustomInstrumentRunMode::Instrument,
                        instrument_base_note_offset: slot.instrument_base_note_offset,
                        pad_note: slot.pad_note,
                        choke_group: slot.choke_group,
                        gain: slot.gain,
                        pan: slot.pan.clamp(-1.0, 1.0),
                        mute: slot.mute,
                        solo: slot.solo,
                        max_polyphony,
                        param_plocks: slot.param_plocks.unwrap_or_default(),
                        instrument_slot,
                        effect_slots: slot
                            .effect_slots
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_slots),
                        effect_descriptors: slot
                            .effect_descriptors
                            .unwrap_or_else(EffectDescriptor::default_full_chain),
                        custom_effect_names: slot
                            .custom_effect_names
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_names),
                        track_sound_state: slot.track_sound_state.unwrap_or_default(),
                        sample_id,
                    });
                    rack_slot_nodes.push(RackSlotNodeIds {
                        sampler_pool_id: Some(pool_id),
                        engine_id: None,
                        sampler_voice_lids: voices.voice_lids,
                        sampler_ids: voices.sampler_ids,
                        sampler_gatepitch_ids: voices.gatepitch_ids,
                        sampler_modulator_ids: voices.modulator_ids,
                        slot_sum_l_id: mixer.slot_sum_l_id,
                        slot_sum_r_id: mixer.slot_sum_r_id,
                        slot_pan_id: mixer.slot_pan_id,
                    });
                }
                RackSlotInstrumentBuildSpec::Custom(custom) => {
                    let route_idx = rack_slot_pool_index(idx, slot_idx).ok_or_else(|| {
                        format!("Rack slot {} has no route-consumer identity", slot_idx + 1)
                    })?;
                    self.ensure_custom_engine_runtime(
                        custom.engine_id,
                        custom.instrument_name,
                        custom.manifest,
                        custom.lib,
                    )?;
                    self.connect_engine_to_track(
                        custom.engine_id,
                        route_idx,
                        idx,
                        &slot_name,
                        mixer.slot_sum_l_id,
                        mixer.slot_sum_r_id,
                        shell.mod_out_id,
                        shell.mod_in_clip_ids,
                    )?;
                    let engine = self.app.graph.engine_node_ids[custom.engine_id]
                        .as_ref()
                        .ok_or_else(|| {
                            format!(
                                "Rack custom slot '{}' failed to initialize engine {}",
                                custom.instrument_name, custom.engine_id
                            )
                        })?;
                    let desc = lisp_host::instrument_descriptor_from_manifest(
                        custom.instrument_name,
                        custom.manifest,
                    );
                    let node_id = first_graph_node_identity(&engine.synth_ids);
                    let modulator_node_id = first_graph_node_identity(&engine.modulator_ids);
                    let mut instrument_slot = slot.instrument_slot.unwrap_or_else(|| {
                        EffectSlotSnapshot::new_default_with_modulator(
                            &desc,
                            node_id,
                            modulator_node_id,
                        )
                    });
                    instrument_slot.sync_to_descriptor_with_modulator(
                        &desc,
                        node_id,
                        modulator_node_id,
                    );
                    let mut sound_state = slot.track_sound_state.unwrap_or_default();
                    sound_state.engine_id = Some(custom.engine_id);
                    rack_slot_snapshots.push(RackSlotSnapshot {
                        instrument_type: InstrumentType::Custom,
                        instrument_run_mode: custom.run_mode,
                        instrument_base_note_offset: slot.instrument_base_note_offset,
                        pad_note: slot.pad_note,
                        choke_group: slot.choke_group,
                        gain: slot.gain,
                        pan: slot.pan.clamp(-1.0, 1.0),
                        mute: slot.mute,
                        solo: slot.solo,
                        max_polyphony,
                        param_plocks: slot.param_plocks.unwrap_or_default(),
                        instrument_slot,
                        effect_slots: slot
                            .effect_slots
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_slots),
                        effect_descriptors: slot
                            .effect_descriptors
                            .unwrap_or_else(EffectDescriptor::default_full_chain),
                        custom_effect_names: slot
                            .custom_effect_names
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_names),
                        track_sound_state: sound_state,
                        sample_id: None,
                    });
                    rack_slot_nodes.push(RackSlotNodeIds {
                        sampler_pool_id: None,
                        engine_id: Some(custom.engine_id),
                        sampler_voice_lids: Vec::new(),
                        sampler_ids: Vec::new(),
                        sampler_gatepitch_ids: Vec::new(),
                        sampler_modulator_ids: Vec::new(),
                        slot_sum_l_id: mixer.slot_sum_l_id,
                        slot_sum_r_id: mixer.slot_sum_r_id,
                        slot_pan_id: mixer.slot_pan_id,
                    });
                }
            }
        }

        let rack_track = RackTrackSnapshot {
            routing,
            slots: rack_slot_snapshots,
            macros: crate::sequencer::default_rack_macros(),
            runtime_macro_values: None,
            runtime_macro_track: 0,
        };
        self.finish_rack_track_registration(idx, track_name, shell, rack_slot_nodes, rack_track)?;
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_empty_rack_track(&mut self) -> Result<usize, String> {
        self.add_rack_track(
            "Drum Rack",
            RackRouting::ByPitch,
            Vec::<RackSlotBuildSpec<'_>>::new(),
        )
    }

    pub fn add_empty_layer_rack_track(&mut self) -> Result<usize, String> {
        self.add_rack_track(
            "Layer Rack",
            RackRouting::Broadcast,
            Vec::<RackSlotBuildSpec<'_>>::new(),
        )
    }

    pub fn group_track_to_instrument_rack(&mut self, track: usize) -> Result<(), String> {
        let instrument_type = self
            .app
            .graph
            .track_instrument_types
            .get(track)
            .copied()
            .ok_or_else(|| format!("Invalid track index {}", track + 1))?;
        if !matches!(
            instrument_type,
            InstrumentType::Sampler | InstrumentType::Custom
        ) {
            return Err("Only sampler and custom-instrument tracks can be grouped".to_string());
        }
        self.app.state.validate_group_flat_track_to_rack(track)?;
        let rack_locator = FxChainLocator::RackSlot { track, slot: 0 };
        if self
            .app
            .editor
            .effect_chain_leases
            .contains_host(rack_locator)
        {
            return Err("Rack slot effect-chain host is already in use".to_string());
        }
        let old_nodes = self.app.graph.track_node_ids[track].clone();
        let old_host = self.app.fx_chain_host(FxChainLocator::Track(track))?;
        let descriptors = self.app.graph.effect_descriptors[track].clone();
        let custom_effect_names = descriptors
            .iter()
            .enumerate()
            .map(|(slot_idx, descriptor)| {
                let active = self.app.state.pattern.effect_chains[track][slot_idx]
                    .node_id
                    .load(Ordering::Relaxed)
                    != 0;
                active.then(|| {
                    EffectDescriptor::builtin_insert_project_name(&descriptor.name)
                        .unwrap_or_else(|| descriptor.name.clone())
                })
            })
            .collect::<Vec<_>>();
        let track_name = self.app.tracks[track].clone();
        let instrument_run_mode = self.app.graph.track_instrument_run_modes[track];
        let engine_id = self.app.graph.track_engine_ids[track];
        if instrument_type == InstrumentType::Custom {
            let engine_id =
                engine_id.ok_or_else(|| "Custom track has no engine binding".to_string())?;
            self.validated_engine_route_ids_for_track(engine_id, track)?;
        }
        if !self.app.state.save_current_pattern_snapshot(
            self.app.tracks.len(),
            &self.app.graph.track_buffer_ids,
            &self.app.graph.track_sample_rates,
            &self.app.tracks,
            &self.app.graph.track_instrument_types,
        ) {
            return Err("Failed to save the active track pattern before grouping".to_string());
        }
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let mixer = self.create_rack_slot_mixer(
            &format!("{}_rack1", track_name),
            old_nodes.voice_sum_id,
            old_nodes.voice_sum_r_id,
            1.0,
            0.0,
            false,
            false,
        )?;

        let rack_nodes = match instrument_type {
            InstrumentType::Sampler => {
                let pool_id = rack_slot_pool_index(track, 0)
                    .ok_or_else(|| "Rack sampler pool unavailable".to_string())?;
                let buffer_id = self.app.graph.track_buffer_ids[track];
                let sample_rate = self.app.graph.track_sample_rates[track];
                let voices = self.build_sampler_voices(
                    pool_id,
                    &format!("{}_rack1", track_name),
                    buffer_id,
                    sample_rate,
                    mixer.slot_sum_l_id,
                    mixer.slot_sum_r_id,
                    old_nodes.mod_in_clip_ids,
                    old_nodes.sampler_ids.len().max(1),
                )?;
                self.publish_sampler_voice_runtime(
                    pool_id,
                    &voices.voice_lids,
                    &voices.sampler_ids,
                    &voices.gatepitch_ids,
                    &voices.modulator_ids,
                );
                self.delete_sampler_voice_nodes(&old_nodes);
                self.clear_sampler_runtime_pool(track);
                RackSlotNodeIds {
                    sampler_pool_id: Some(pool_id),
                    engine_id: None,
                    sampler_voice_lids: voices.voice_lids,
                    sampler_ids: voices.sampler_ids,
                    sampler_gatepitch_ids: voices.gatepitch_ids,
                    sampler_modulator_ids: voices.modulator_ids,
                    slot_sum_l_id: mixer.slot_sum_l_id,
                    slot_sum_r_id: mixer.slot_sum_r_id,
                    slot_pan_id: mixer.slot_pan_id,
                }
            }
            InstrumentType::Custom => {
                let engine_id = self.app.graph.track_engine_ids[track]
                    .ok_or_else(|| "Custom track has no engine binding".to_string())?;
                self.rewire_engine_route_output_for_track(
                    engine_id,
                    track,
                    old_nodes.voice_sum_id,
                    old_nodes.voice_sum_r_id,
                    mixer.slot_sum_l_id,
                    mixer.slot_sum_r_id,
                )?;
                let route_idx = rack_slot_pool_index(track, 0)
                    .ok_or_else(|| "Rack custom route unavailable".to_string())?;
                self.move_engine_route_to_rack_consumer(engine_id, track, route_idx)?;
                RackSlotNodeIds {
                    sampler_pool_id: None,
                    engine_id: Some(engine_id),
                    sampler_voice_lids: Vec::new(),
                    sampler_ids: Vec::new(),
                    sampler_gatepitch_ids: Vec::new(),
                    sampler_modulator_ids: Vec::new(),
                    slot_sum_l_id: mixer.slot_sum_l_id,
                    slot_sum_r_id: mixer.slot_sum_r_id,
                    slot_pan_id: mixer.slot_pan_id,
                }
            }
            _ => unreachable!(),
        };

        let rack = self
            .app
            .state
            .group_flat_track_to_rack(
                track,
                instrument_type,
                instrument_run_mode,
                engine_id,
                &descriptors,
                &custom_effect_names,
            )
            .ok_or_else(|| "Failed to move flat-track state into rack".to_string())?;
        self.app.graph.track_node_ids[track].rack_slots = vec![rack_nodes];
        self.app.graph.track_node_ids[track].rack_signature = Some(rack_topology_signature(&rack));
        self.app.graph.track_node_ids[track].sampler_ids.clear();
        self.app.graph.track_node_ids[track]
            .sampler_gatepitch_ids
            .clear();
        self.app.graph.track_node_ids[track]
            .sampler_modulator_ids
            .clear();
        self.app.graph.track_voice_lids[track].clear();
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.track_instrument_types[track] = InstrumentType::Rack;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.instrument_descriptors[track] = EffectDescriptor::empty_custom_slot();
        self.app.graph.effect_descriptors[track] = EffectDescriptor::default_full_chain();

        let new_host = self.app.fx_chain_host(rack_locator)?;
        rewire_fx_chain(self.app.graph.lg.0, &old_host, &new_host);
        connect_fx_chain_gap(
            self.app.graph.lg.0,
            StereoEndpoint {
                node_id: old_nodes.pan_id,
                channels: 2,
            },
            ChainSuccessor::StereoNode(StereoEndpoint {
                node_id: old_nodes.delay_id,
                channels: 2,
            }),
        );
        self.app
            .editor
            .effect_chain_leases
            .move_host(FxChainLocator::Track(track), rack_locator)?;
        self.app.tracks[track] = format!("Rack {track_name}");
        self.app.set_rack_selected_slot(track, 0);
        self.publish_rack_slot_panner_runtime(track);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        self.app.push_all_restored_defaults();
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn replace_track_instrument_container_with_rack(
        &mut self,
        track: usize,
        mut rack: RackTrackSnapshot,
        display_name: &str,
    ) -> Result<(), String> {
        let previous_type = self
            .app
            .graph
            .track_instrument_types
            .get(track)
            .copied()
            .ok_or_else(|| format!("Invalid track index {}", track + 1))?;
        let old_nodes = self.app.graph.track_node_ids[track].clone();
        let old_flat_engine = (previous_type == InstrumentType::Custom)
            .then(|| self.app.graph.track_engine_ids[track])
            .flatten();
        self.validate_rack_slot_graph_rebuild(track, &rack)?;

        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        if previous_type == InstrumentType::Rack {
            self.delete_rack_effect_chains(track, batch.serial)?;
        } else {
            match previous_type {
                InstrumentType::Sampler => {
                    self.delete_sampler_voice_nodes(&old_nodes);
                    self.clear_sampler_runtime_pool(track);
                }
                InstrumentType::Custom => {
                    if let Some(engine_id) = old_flat_engine {
                        self.delete_engine_route_for_track(engine_id, track, track);
                    }
                }
                _ => {
                    return Err("Only sampler, custom, or rack tracks can load a Sound".to_string());
                }
            }
        }

        if !self
            .app
            .state
            .replace_instrument_container_with_rack(track, rack.clone())
        {
            return Err("Failed to replace track rack state".to_string());
        }
        self.app.graph.track_instrument_types[track] = InstrumentType::Rack;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.track_node_ids[track].sampler_ids.clear();
        self.app.graph.track_node_ids[track]
            .sampler_gatepitch_ids
            .clear();
        self.app.graph.track_node_ids[track]
            .sampler_modulator_ids
            .clear();
        self.app.graph.track_voice_lids[track].clear();
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.instrument_descriptors[track] = EffectDescriptor::empty_custom_slot();

        let bindings = self.rebuild_rack_slot_graph(track, &mut rack)?;
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track, &bindings)
        {
            return Err("Failed to bind loaded Sound instruments".to_string());
        }
        if let Some(engine_id) = old_flat_engine {
            if !self.engine_is_still_referenced(engine_id) {
                self.delete_engine_runtime(engine_id);
            }
        }
        self.app.tracks[track] = display_name.to_string();
        self.app.set_rack_selected_slot(track, 0);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn replace_rack_track_with_sampler(
        &mut self,
        track: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err(format!("Track {} is not an instrument rack", track + 1));
        }
        if buffer_id < 0 {
            return Err("Retained sampler buffer is invalid".to_string());
        }
        let track_id = self.app.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        self.app.state.validate_sampler_slot_reset_target(track)?;
        let track_nodes = self.app.graph.track_node_ids.get(track).cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        let (sampler_nodes, sampler_connections) = sampler_voice_build_capacities(MAX_VOICES);
        let required_commands = (sampler_nodes + sampler_connections)
            .checked_mul(2)
            .ok_or_else(|| "Rack-to-sampler graph capacity overflow".to_string())?;
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Rack-to-sampler conversion",
        )?;

        let descriptor = EffectDescriptor::builtin_sampler();
        let old_engine_ids = self.rack_engine_ids_for_track(track);
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let voices = self.build_sampler_voices(
            track,
            sample_name,
            buffer_id,
            sample_rate,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.delete_rack_effect_chains(track, batch.serial)?;
        self.retire_rack_slot_graph_generation(track);
        self.publish_sampler_voice_runtime(
            track,
            &voices.voice_lids,
            &voices.sampler_ids,
            &voices.gatepitch_ids,
            &voices.modulator_ids,
        );

        let node_id = first_graph_node_identity(&voices.sampler_ids);
        let modulator_node_id = first_graph_node_identity(&voices.modulator_ids);
        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.rack_slots.clear();
        live_nodes.rack_signature = None;
        live_nodes.sampler_ids = voices.sampler_ids;
        live_nodes.sampler_gatepitch_ids = voices.gatepitch_ids;
        live_nodes.sampler_modulator_ids = voices.modulator_ids;
        self.app.graph.track_voice_lids[track] = voices.voice_lids;
        self.app.graph.track_buffer_ids[track] = buffer_id;
        self.app.graph.track_sample_rates[track] = sample_rate.max(1);
        self.app.graph.track_instrument_types[track] = InstrumentType::Sampler;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self.app.state.reset_sampler_slot_all_patterns(
            track,
            &descriptor,
            node_id,
            modulator_node_id,
            (buffer_id, sample_name.to_string(), sample_rate.max(1)),
        ).expect("sampler reset target was validated before graph mutation");
        drop(batch);

        self.reap_excess_rack_teardowns();
        for engine_id in old_engine_ids {
            if !self.engine_is_still_referenced(engine_id) {
                lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
            }
        }
        self.app.tracks[track] = sample_name.to_string();
        self.app.reset_sampler_bpm_for_analysis(track);
        self.app.publish_sampler_analysis_runtime(track);
        self.app.device_registry.clear_rack_track(track_id);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub fn add_sampler_rack_track(
        &mut self,
        sample_paths: &[std::path::PathBuf],
    ) -> Result<usize, String> {
        if sample_paths.is_empty() {
            return Err("Rack track creation requires at least one sample".to_string());
        }
        if sample_paths.len() > MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }

        let mut loaded_slots = Vec::with_capacity(sample_paths.len());
        for path in sample_paths {
            let loaded = crate::sampler::load_wav_buffer(self.app.graph.lg.0, path)?;
            self.app.submit_sample_analysis(&loaded);
            let sample_name =
                crate::sample_db::display_title_for_sample_path(path).unwrap_or(loaded.name);
            loaded_slots.push((
                path.clone(),
                loaded.buffer_id,
                loaded.sample_rate,
                sample_name,
            ));
        }
        let track_name = if loaded_slots.len() == 1 {
            format!("Rack {}", loaded_slots[0].3)
        } else {
            "Layer Rack".to_string()
        };
        let per_slot_max_polyphony = appended_rack_slot_max_polyphony(&[]);
        let specs: Vec<RackSlotBuildSpec<'_>> = loaded_slots
            .iter()
            .map(
                |(_, buffer_id, sample_rate, sample_name)| RackSlotBuildSpec {
                    instrument: RackSlotInstrumentBuildSpec::Sampler(RackSamplerBuildSpec {
                        buffer_id: *buffer_id,
                        sample_rate: *sample_rate,
                        sample_name: sample_name.clone(),
                    }),
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: per_slot_max_polyphony,
                    param_plocks: None,
                    instrument_slot: None,
                    effect_slots: None,
                    effect_descriptors: None,
                    custom_effect_names: None,
                    track_sound_state: None,
                },
            )
            .collect();

        let idx = self.add_rack_track(&track_name, RackRouting::Broadcast, specs)?;
        for (path, buffer_id, _, sample_name) in loaded_slots {
            self.app
                .register_loaded_sample_path(&sample_name, buffer_id, path);
        }
        Ok(idx)
    }

    pub fn add_sampler_drum_rack_track(
        &mut self,
        wav_path: &Path,
        pad_note: i32,
    ) -> Result<usize, String> {
        if !validate_drum_rack_pad_note(pad_note) {
            return Err(format!("Unsupported drum rack pad note {pad_note}"));
        }
        let loaded = crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let sample_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        let specs = vec![RackSlotBuildSpec {
            instrument: RackSlotInstrumentBuildSpec::Sampler(RackSamplerBuildSpec {
                buffer_id: loaded.buffer_id,
                sample_rate: loaded.sample_rate,
                sample_name: sample_name.clone(),
            }),
            instrument_base_note_offset: 0.0,
            pad_note: Some(pad_note),
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: DEFAULT_DRUM_SLOT_MAX_POLYPHONY,
            param_plocks: None,
            instrument_slot: None,
            effect_slots: None,
            effect_descriptors: None,
            custom_effect_names: None,
            track_sound_state: None,
        }];
        let idx = self.add_rack_track("Drum Rack", RackRouting::ByPitch, specs)?;
        self.app.register_loaded_sample_path(
            &sample_name,
            loaded.buffer_id,
            wav_path.to_path_buf(),
        );
        Ok(idx)
    }

    pub fn add_sampler_slot_to_rack(
        &mut self,
        track_idx: usize,
        wav_path: &Path,
    ) -> Result<usize, String> {
        let loaded = crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let sample_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        let slot_idx = self.add_sampler_slot_to_rack_buffer(
            track_idx,
            loaded.buffer_id,
            loaded.sample_rate,
            &sample_name,
        )?;
        self.app.register_loaded_sample_path(
            &sample_name,
            loaded.buffer_id,
            wav_path.to_path_buf(),
        );
        Ok(slot_idx)
    }

    pub fn add_sampler_slot_to_rack_buffer(
        &mut self,
        track_idx: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<usize, String> {
        let (rack, slot_idx) = self.rack_slot_append_target(track_idx)?;
        if rack.routing == RackRouting::ByPitch {
            return Err("Add samples to a drum rack pad, not the rack chain".to_string());
        }
        let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
            return Err(format!(
                "Rack sampler pool unavailable for track {track_idx} slot {slot_idx}"
            ));
        };
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .ok_or_else(|| format!("Track {} has no graph nodes", track_idx + 1))?
            .clone();
        let slot_name = format!("{}_rack{}", self.app.tracks[track_idx], slot_idx + 1);
        let mixer = self.create_rack_slot_mixer(
            &slot_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            1.0,
            0.0,
            false,
            rack.slots.iter().any(|slot| slot.solo),
        )?;
        let max_polyphony = appended_rack_slot_max_polyphony(&rack.slots);
        let voices = self.build_sampler_voices(
            pool_id,
            &slot_name,
            buffer_id,
            sample_rate,
            mixer.slot_sum_l_id,
            mixer.slot_sum_r_id,
            track_nodes.mod_in_clip_ids,
            max_polyphony,
        )?;
        self.publish_sampler_voice_runtime(
            pool_id,
            &voices.voice_lids,
            &voices.sampler_ids,
            &voices.gatepitch_ids,
            &voices.modulator_ids,
        );

        let mut instrument_slot = EffectSlotSnapshot::new_default_with_modulator(
            &EffectDescriptor::builtin_sampler(),
            first_graph_node_identity(&voices.sampler_ids),
            first_graph_node_identity(&voices.modulator_ids),
        );
        instrument_slot.sync_to_descriptor_with_modulator(
            &EffectDescriptor::builtin_sampler(),
            first_graph_node_identity(&voices.sampler_ids),
            first_graph_node_identity(&voices.modulator_ids),
        );
        let rack_slot = RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            pad_note: None,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot,
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((buffer_id, sample_name.to_string(), sample_rate)),
        };
        let rack_slot_nodes = RackSlotNodeIds {
            sampler_pool_id: Some(pool_id),
            engine_id: None,
            sampler_voice_lids: voices.voice_lids,
            sampler_ids: voices.sampler_ids,
            sampler_gatepitch_ids: voices.gatepitch_ids,
            sampler_modulator_ids: voices.modulator_ids,
            slot_sum_l_id: mixer.slot_sum_l_id,
            slot_sum_r_id: mixer.slot_sum_r_id,
            slot_pan_id: mixer.slot_pan_id,
        };
        self.app.graph.track_node_ids[track_idx]
            .rack_slots
            .push(rack_slot_nodes);
        self.publish_rack_slot_panner_runtime(track_idx);
        self.app.set_rack_selected_slot(track_idx, slot_idx);
        self.app.state.append_rack_slot_for_all_pattern_snapshots(
            track_idx,
            rack.routing,
            rack_slot,
        );
        self.refresh_rack_signature_from_live_state(track_idx);
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        Ok(slot_idx)
    }

    pub fn delete_rack_slot(&mut self, track_idx: usize, slot_idx: usize) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let mut rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        if slot_idx >= rack.slots.len() {
            return Err("Invalid rack layer".to_string());
        }
        let active_effect_slots = rack.slots[slot_idx]
            .effect_slots
            .iter()
            .enumerate()
            .filter_map(|(idx, effect)| (effect.node_id != 0).then_some(idx))
            .collect::<Vec<_>>();
        for effect_slot in active_effect_slots {
            self.app
                .delete_rack_slot_effect_slot(track_idx, slot_idx, effect_slot)?;
        }
        self.app.editor.effect_chain_leases.retire_host(
            FxChainLocator::RackSlot {
                track: track_idx,
                slot: slot_idx,
            },
            0,
        )?;
        rack.slots.remove(slot_idx);
        let bindings = self.rebuild_rack_slot_graph(track_idx, &mut rack)?;
        if !self
            .app
            .state
            .remove_rack_slot_from_all_pattern_snapshots(track_idx, slot_idx)
        {
            return Err("Failed to remove rack layer from all patterns".to_string());
        }
        self.app
            .editor
            .effect_chain_leases
            .reindex_rack_slots_after_delete(track_idx, slot_idx);
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
        {
            return Err("Failed to sync rack layer bindings to all patterns".to_string());
        }
        if rack.routing == RackRouting::ByPitch {
            let next_pad = rack
                .slots
                .get(slot_idx.min(rack.slots.len().saturating_sub(1)))
                .and_then(|slot| slot.pad_note)
                .unwrap_or(DRUM_RACK_FIRST_PAD_NOTE);
            self.app.set_rack_selected_pad_note(track_idx, next_pad);
        } else {
            let next_selection = if rack.slots.is_empty() {
                0
            } else {
                slot_idx.min(rack.slots.len() - 1)
            };
            self.app.set_rack_selected_slot(track_idx, next_selection);
        }
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        self.app.push_all_restored_defaults();
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn add_custom_slot_to_rack(
        &mut self,
        track_idx: usize,
        instrument_name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<usize, String> {
        let (rack, slot_idx) = self.rack_slot_append_target(track_idx)?;
        if rack.routing == RackRouting::ByPitch {
            return Err("Add instruments to a drum rack pad, not the rack chain".to_string());
        }
        let route_idx = rack_slot_pool_index(track_idx, slot_idx)
            .ok_or_else(|| format!("Rack slot {} has no route-consumer identity", slot_idx + 1))?;

        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.ensure_custom_engine_runtime(engine_id, instrument_name, manifest, lib)?;
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .ok_or_else(|| format!("Track {} has no graph nodes", track_idx + 1))?
            .clone();
        let slot_name = format!("{}_rack{}", self.app.tracks[track_idx], slot_idx + 1);
        let mixer = self.create_rack_slot_mixer(
            &slot_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            1.0,
            0.0,
            false,
            rack.slots.iter().any(|slot| slot.solo),
        )?;
        self.connect_engine_to_track(
            engine_id,
            route_idx,
            track_idx,
            &slot_name,
            mixer.slot_sum_l_id,
            mixer.slot_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;

        let engine = self.app.graph.engine_node_ids[engine_id]
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "Rack custom slot '{}' failed to initialize engine {}",
                    instrument_name, engine_id
                )
            })?;
        let desc = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let node_id = first_graph_node_identity(&engine.synth_ids);
        let modulator_node_id = first_graph_node_identity(&engine.modulator_ids);
        let mut instrument_slot =
            EffectSlotSnapshot::new_default_with_modulator(&desc, node_id, modulator_node_id);
        instrument_slot.sync_to_descriptor_with_modulator(&desc, node_id, modulator_node_id);

        let rack_slot = RackSlotSnapshot {
            instrument_type: InstrumentType::Custom,
            instrument_run_mode: run_mode,
            instrument_base_note_offset: 0.0,
            pad_note: None,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: MAX_VOICES,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot,
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState {
                engine_id: Some(engine_id),
                loaded_preset: Some(instrument_name.to_string()),
                dirty: false,
            },
            sample_id: None,
        };
        let rack_slot_nodes = RackSlotNodeIds {
            sampler_pool_id: None,
            engine_id: Some(engine_id),
            sampler_voice_lids: Vec::new(),
            sampler_ids: Vec::new(),
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            slot_sum_l_id: mixer.slot_sum_l_id,
            slot_sum_r_id: mixer.slot_sum_r_id,
            slot_pan_id: mixer.slot_pan_id,
        };
        self.app.graph.track_node_ids[track_idx]
            .rack_slots
            .push(rack_slot_nodes);
        self.publish_rack_slot_panner_runtime(track_idx);
        self.app.set_rack_selected_slot(track_idx, slot_idx);
        self.app.state.append_rack_slot_for_all_pattern_snapshots(
            track_idx,
            rack.routing,
            rack_slot,
        );
        self.refresh_rack_signature_from_live_state(track_idx);
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        Ok(slot_idx)
    }

    fn replace_layer_rack_slot_source(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        replacement: RackSlotSnapshot,
    ) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let mut rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        if rack.routing != RackRouting::Broadcast {
            return Err("Replace drum rack instruments on their pad".to_string());
        }
        let existing = rack
            .slots
            .get(slot_idx)
            .cloned()
            .ok_or_else(|| "Invalid rack layer".to_string())?;
        rack.slots[slot_idx] = preserve_rack_slot_configuration(replacement, &existing);

        let bindings = self.rebuild_rack_slot_graph(track_idx, &mut rack)?;
        if !self.app.state.replace_rack_slot_source_in_current_pattern(
            track_idx,
            slot_idx,
            rack.slots[slot_idx].clone(),
        ) {
            return Err("Failed to replace rack layer source".to_string());
        }
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
        {
            return Err("Failed to sync rack layer bindings to all patterns".to_string());
        }
        self.app.set_rack_selected_slot(track_idx, slot_idx);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        self.app.push_all_restored_defaults();
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn replace_rack_slot_with_sampler(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        wav_path: &Path,
    ) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        if rack.routing != RackRouting::Broadcast || rack.slots.get(slot_idx).is_none() {
            return Err("Invalid instrument rack layer".to_string());
        }

        let loaded = crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let sample_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        self.replace_rack_slot_with_sampler_buffer(
            track_idx,
            slot_idx,
            loaded.buffer_id,
            loaded.sample_rate,
            &sample_name,
        )?;
        self.app.register_loaded_sample_path(
            &sample_name,
            loaded.buffer_id,
            wav_path.to_path_buf(),
        );
        Ok(())
    }

    pub fn replace_rack_slot_with_sampler_buffer(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let replacement = RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            pad_note: None,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: MAX_VOICES,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_empty(),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((buffer_id, sample_name.to_string(), sample_rate)),
        };
        self.replace_layer_rack_slot_source(track_idx, slot_idx, replacement)
    }

    pub fn replace_rack_slot_with_custom(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        instrument_name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<(), String> {
        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let replacement = RackSlotSnapshot {
            instrument_type: InstrumentType::Custom,
            instrument_run_mode: run_mode,
            instrument_base_note_offset: 0.0,
            pad_note: None,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: MAX_VOICES,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_default_with_modulator(&descriptor, 0, 0),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState {
                engine_id: Some(engine_id),
                loaded_preset: Some(instrument_name.to_string()),
                dirty: false,
            },
            sample_id: None,
        };
        self.replace_layer_rack_slot_source(track_idx, slot_idx, replacement)
    }

    fn add_or_replace_drum_rack_slot_source(
        &mut self,
        track_idx: usize,
        pad_note: i32,
        mut replacement: RackSlotSnapshot,
    ) -> Result<usize, String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        if !validate_drum_rack_pad_note(pad_note) {
            return Err(format!("Unsupported drum rack pad note {pad_note}"));
        }
        let mut rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        if rack.routing != RackRouting::ByPitch {
            return Err("Current rack is not a drum rack".to_string());
        }
        replacement.pad_note = Some(pad_note);
        let replacing_slot_idx = rack
            .slots
            .iter()
            .position(|slot| slot.pad_note == Some(pad_note));
        let slot_idx = if let Some(slot_idx) = replacing_slot_idx {
            let existing = rack.slots[slot_idx].clone();
            rack.slots[slot_idx] = preserve_rack_slot_configuration(replacement, &existing);
            slot_idx
        } else {
            if rack.slots.len() >= MAX_RACK_SLOTS {
                return Err(format!(
                    "Rack tracks support at most {MAX_RACK_SLOTS} slots"
                ));
            }
            rack.slots.push(replacement);
            rack.slots.len() - 1
        };

        let bindings = self.rebuild_rack_slot_graph(track_idx, &mut rack)?;
        if replacing_slot_idx.is_some() {
            if !self.app.state.replace_rack_slot_source_in_current_pattern(
                track_idx,
                slot_idx,
                rack.slots[slot_idx].clone(),
            ) {
                return Err("Failed to replace drum rack pad source".to_string());
            }
        } else {
            self.app.state.append_rack_slot_for_all_pattern_snapshots(
                track_idx,
                rack.routing,
                rack.slots[slot_idx].clone(),
            );
        }
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
        {
            return Err("Failed to sync drum rack pad bindings to all patterns".to_string());
        }
        self.app.set_rack_selected_pad_note(track_idx, pad_note);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        self.app.push_all_restored_defaults();
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        Ok(slot_idx)
    }

    pub fn add_sampler_slot_to_drum_rack_pad(
        &mut self,
        track_idx: usize,
        wav_path: &Path,
        pad_note: i32,
    ) -> Result<usize, String> {
        let loaded = crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let sample_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        let rack_slot = RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            pad_note: Some(pad_note),
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: DEFAULT_DRUM_SLOT_MAX_POLYPHONY,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_empty(),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((loaded.buffer_id, sample_name.clone(), loaded.sample_rate)),
        };
        let slot_idx = self.add_or_replace_drum_rack_slot_source(track_idx, pad_note, rack_slot)?;
        self.app.register_loaded_sample_path(
            &sample_name,
            loaded.buffer_id,
            wav_path.to_path_buf(),
        );
        Ok(slot_idx)
    }

    pub fn add_custom_slot_to_drum_rack_pad(
        &mut self,
        track_idx: usize,
        pad_note: i32,
        instrument_name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        _lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<usize, String> {
        let desc = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let rack_slot = RackSlotSnapshot {
            instrument_type: InstrumentType::Custom,
            instrument_run_mode: run_mode,
            instrument_base_note_offset: 0.0,
            pad_note: Some(pad_note),
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: DEFAULT_DRUM_SLOT_MAX_POLYPHONY,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_default_with_modulator(&desc, 0, 0),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState {
                engine_id: Some(engine_id),
                loaded_preset: Some(instrument_name.to_string()),
                dirty: false,
            },
            sample_id: None,
        };
        self.add_or_replace_drum_rack_slot_source(track_idx, pad_note, rack_slot)
    }

    pub fn hot_reload_instrument(
        &mut self,
        track: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        if track >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if self.app.graph.track_instrument_types[track] != InstrumentType::Custom {
            return Err("Not a custom instrument track".to_string());
        }

        let Some(engine_id) = self.app.graph.track_engine_ids[track] else {
            return Err("Custom track has no engine binding".to_string());
        };

        self.rebuild_custom_engine_runtime(engine_id, manifest, lib)?;

        for bound_track in 0..self.app.tracks.len() {
            if self.app.graph.track_engine_ids.get(bound_track) == Some(&Some(engine_id)) {
                let track_name = self.app.tracks[bound_track].clone();
                self.sync_instrument_slot(bound_track, &track_name, manifest);
            }
        }

        for bound_track in 0..self.app.tracks.len() {
            if self.app.graph.track_engine_ids.get(bound_track) == Some(&Some(engine_id))
                && self
                    .app
                    .graph
                    .track_instrument_run_modes
                    .get(bound_track)
                    .copied()
                    == Some(CustomInstrumentRunMode::FreePatch)
            {
                self.apply_free_patch_idle_voice(bound_track)?;
            }
        }

        Ok(())
    }

    pub fn apply_sample_ids(&mut self, sample_ids: &[(i32, String, u32)]) {
        for (track, (buffer_id, name, sample_rate)) in sample_ids.iter().enumerate() {
            if *buffer_id < 0 {
                continue;
            }
            if track >= self.app.tracks.len() {
                break;
            }
            if !self.app.is_sampler_track(track) {
                continue;
            }
            self.send_sample_to_all_voices(track, *buffer_id, *sample_rate);
            self.app.graph.track_buffer_ids[track] = *buffer_id;
            if let Some(track_sample_rate) = self.app.graph.track_sample_rates.get_mut(track) {
                *track_sample_rate = *sample_rate;
            }
            self.app.tracks[track] = name.clone();
            self.app
                .sync_sampler_path_from_sample(track, *buffer_id, name);
            self.app.publish_sampler_analysis_runtime(track);
        }
        if let Err(error) = self.sync_live_rack_tracks_from_pattern_state() {
            self.app.editor.status_message = Some((
                format!("Pattern rack sync failed: {error}"),
                std::time::Instant::now(),
            ));
        }
    }

    pub fn sync_live_rack_tracks_from_pattern_state(&mut self) -> Result<(), String> {
        let rack_tracks = self.app.state.pattern.rack_tracks.lock().unwrap().clone();
        let mut rebuilt_any = false;
        let mut applied_in_place_any = false;
        for (track_idx, rack) in rack_tracks.into_iter().enumerate() {
            if track_idx >= self.app.tracks.len() {
                break;
            }
            if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
                continue;
            }
            let Some(mut rack) = rack else {
                continue;
            };
            let incoming_signature = rack_topology_signature(&rack);
            let live_signature = self.app.graph.track_node_ids[track_idx]
                .rack_signature
                .clone();
            let in_place = live_signature.as_ref() == Some(&incoming_signature)
                && self
                    .validate_rack_slot_graph_rebuild(track_idx, &rack)
                    .is_ok();
            let bindings = if in_place {
                applied_in_place_any = true;
                self.apply_rack_scene_state_in_place(track_idx, &mut rack)?
            } else {
                rebuilt_any = true;
                self.rebuild_rack_slot_graph(track_idx, &mut rack)?
            };
            if std::env::var_os("TINYSEQ_LOG_RACK_SYNC").is_some() {
                eprintln!(
                    "rack sync track {track_idx}: {}",
                    if in_place { "in-place" } else { "rebuild" }
                );
            }
            if let Some(live_rack_track) = self
                .app
                .state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get_mut(track_idx)
            {
                *live_rack_track = Some(rack);
            }
            if !self
                .app
                .state
                .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
            {
                return Err(format!(
                    "Failed to sync rack bindings for track {}",
                    track_idx + 1
                ));
            }
        }
        if rebuilt_any {
            self.app.state.schedule_mod_resync();
            self.app.state.request_all_accumulator_resets();
            self.app.state.publish_scheduler_snapshot();
            self.app
                .state
                .transport
                .topology_epoch
                .fetch_add(1, Ordering::Relaxed);
        } else if applied_in_place_any {
            self.app.state.publish_scheduler_snapshot();
        }
        Ok(())
    }

    pub fn clear_all_tracks(&mut self) {
        self.force_reap_all_rack_teardowns();
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let old_track_count = self.app.tracks.len();

        for track_idx in 0..old_track_count {
            if let Err(error) = self.delete_rack_effect_chains(track_idx, batch.serial) {
                self.app.editor.status_message = Some((error, std::time::Instant::now()));
            }
            for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
                ..self.app.state.pattern.effect_chains[track_idx].len()
            {
                let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
                let node_id = slot.node_id.load(Ordering::Relaxed);
                let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
                if node_id == 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id as i32);
                    if modulator_node_id != 0 {
                        crate::audiograph::delete_node(
                            self.app.graph.lg.0,
                            modulator_node_id as i32,
                        );
                    }
                }
            }
        }

        for engine in self.app.graph.engine_node_ids.iter_mut().flatten() {
            for routes in &engine.route_gain_ids {
                for route_pair in routes {
                    for &route_id in route_pair {
                        if route_id <= 0 {
                            continue;
                        }
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
            for routes in &engine.ext_route_gain_ids {
                for route_ids in routes {
                    for &route_id in route_ids {
                        if route_id > 0 {
                            unsafe {
                                crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                            }
                        }
                    }
                }
            }
            for &node_id in &engine.synth_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                }
            }
            for &node_id in &engine.modulator_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                }
            }
            for &node_id in &engine.gatepitch_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                }
            }
        }

        for track in self.app.graph.track_node_ids.iter().rev() {
            for rack_slot in &track.rack_slots {
                self.delete_rack_slot_nodes(rack_slot);
            }
            for &sampler_id in &track.sampler_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
                }
            }
            for &modulator_id in &track.sampler_modulator_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
                }
            }
            for &gatepitch_id in &track.sampler_gatepitch_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, gatepitch_id);
                }
            }
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, track.send_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.mod_env_id);
                for &mod_in_clip_id in &track.mod_in_clip_ids {
                    crate::audiograph::delete_node(self.app.graph.lg.0, mod_in_clip_id);
                }
                crate::audiograph::delete_node(self.app.graph.lg.0, track.mod_out_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.delay_id);
                if track.filter_id != 0 {
                    crate::audiograph::delete_node(self.app.graph.lg.0, track.filter_id);
                }
                crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, track.pan_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.pan_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_r_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_id);
            }
        }

        self.app.tracks.clear();
        self.app.track_registry = crate::sequencer::TrackRegistry::default();
        self.app.track_colors.clear();
        self.app.track_collapsed.clear();
        self.app.sampler_paths.clear();
        self.app.graph.track_node_ids.clear();
        self.app.graph.applied_mod_routes.clear();
        self.app.graph.track_buffer_ids.clear();
        self.app.graph.track_sample_rates.clear();
        self.app.graph.track_voice_lids.clear();
        self.app.graph.track_instrument_types.clear();
        self.app.graph.track_instrument_run_modes.clear();
        self.app.graph.track_engine_ids.clear();
        self.app.graph.track_synth_node_ids.clear();
        self.app.graph.track_gatepitch_node_ids.clear();
        self.app.graph.engine_node_ids.clear();
        self.app.graph.effect_descriptors.clear();
        self.app.graph.instrument_descriptors.clear();
        self.app.graph.record_armed.clear();
        self.clear_all_rack_sampler_runtime_pools();
        if let Err(error) = self
            .app
            .editor
            .effect_chain_leases
            .retire_tracks(batch.serial)
        {
            self.app.editor.status_message = Some((error, std::time::Instant::now()));
        }

        self.app.ui.cursor_track = 0;
        self.app.ui.cursor_step = 0;
        self.app.ui.pattern_page = 0;
        self.app.ui.focused_region = super::Region::Sidebar;
        self.app.ui.sidebar_tab = super::SidebarTab::Sounds;
        self.app.ui.sidebar_mode = super::SidebarMode::InstrumentPicker;
        self.app.ui.sidebar_search_focused = false;

        self.app.state.clear_live_track_state(old_track_count);
        self.app
            .state
            .transport
            .num_tracks
            .store(0, Ordering::Release);
        self.app.state.replace_pattern_repository(
            vec![crate::sequencer::PatternSnapshot::new_default(0, &[])],
            0,
        );
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
    }

    pub fn clear_all_bus_effect_chains(&mut self) {
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let bus_nodes = self.app.graph.bus_node_ids.clone();

        for bus in &mut self.app.buses {
            let Some(nodes) = bus_nodes.iter().find(|nodes| nodes.id == bus.id) else {
                continue;
            };

            let active_effect_ids = bus
                .effect_slots
                .iter()
                .filter_map(|slot| (slot.node_id != 0).then_some(slot.node_id as i32))
                .collect::<Vec<_>>();
            let active_effect_modulator_ids = bus
                .effect_slots
                .iter()
                .filter_map(|slot| {
                    (slot.modulator_node_id != 0).then_some(slot.modulator_node_id as i32)
                })
                .collect::<Vec<_>>();

            unsafe {
                let mut predecessor_id = nodes.gate_id;
                for effect_id in &active_effect_ids {
                    disconnect_all_ports(self.app.graph.lg.0, predecessor_id, *effect_id);
                    predecessor_id = *effect_id;
                }
                disconnect_all_ports(self.app.graph.lg.0, predecessor_id, nodes.volume_id);
                disconnect_all_ports(self.app.graph.lg.0, nodes.gate_id, nodes.volume_id);

                for effect_id in active_effect_ids {
                    crate::audiograph::delete_node(self.app.graph.lg.0, effect_id);
                    crate::effects::conv_reverb::clear_instance(effect_id);
                }
                for modulator_id in active_effect_modulator_ids {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
                }

                connect_stereo_pair(self.app.graph.lg.0, nodes.gate_id, nodes.volume_id);
            }

            bus.effect_descriptors = super::BusChannelState::default_effect_descriptors();
            bus.effect_slots = super::BusChannelState::default_effect_slots();
            bus.custom_effect_names = vec![None; crate::lisp_host::MAX_CUSTOM_FX];
        }
        if let Err(error) = self
            .app
            .editor
            .effect_chain_leases
            .retire_buses(batch.serial)
        {
            self.app.editor.status_message = Some((error, std::time::Instant::now()));
        }

        self.app.publish_bus_gate_runtime();
    }

    pub fn delete_track(&mut self, track_idx: usize) -> Result<usize, String> {
        if delete_without_shift_enabled() {
            return self.clear_track_in_place(track_idx);
        }
        let old_count = self.app.tracks.len();
        if old_count <= 1 {
            return Err("Cannot delete the last remaining track".to_string());
        }
        if track_idx >= old_count {
            return Err("Invalid track index".to_string());
        }
        self.force_reap_all_rack_teardowns();

        let names = self.app.tracks.clone();
        let buffer_ids = self.app.graph.track_buffer_ids.clone();
        let sample_rates = self.app.graph.track_sample_rates.clone();
        let instrument_types = self.app.graph.track_instrument_types.clone();
        let deleted_engine_id = self.app.graph.track_engine_ids[track_idx];
        let deleted_rack_engine_ids = self.rack_engine_ids_for_track(track_idx);

        let retire_after;
        {
            let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            retire_after = batch.serial;
            self.delete_custom_effect_chain(track_idx)?;
            self.delete_rack_effect_chains(track_idx, retire_after)?;
            self.delete_track_engine_routes(track_idx);

            let track_nodes = self.app.graph.track_node_ids[track_idx].clone();
            self.delete_track_shell(&track_nodes);

            if let Some(engine_id) = deleted_engine_id {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }
            for engine_id in deleted_rack_engine_ids {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }

            self.shift_engine_route_tables_left(track_idx, old_count);
        }

        if !self.app.state.remove_track(
            track_idx,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
            &self.app.graph.effect_descriptors,
        ) {
            return Err("Failed to compact sequencer state for deleted track".to_string());
        }

        self.compact_app_track_vectors(track_idx, retire_after)?;
        self.rebind_live_track_runtime_after_delete();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();

        let new_selected = track_idx.min(self.app.tracks.len().saturating_sub(1));
        self.app.ui.cursor_track = new_selected;
        self.app.ui.cursor_step = self.app.ui.cursor_step.min(
            self.app.state.pattern.track_params[new_selected]
                .get_num_steps()
                .saturating_sub(1),
        );

        Ok(new_selected)
    }

    pub fn clear_track_in_place(&mut self, track_idx: usize) -> Result<usize, String> {
        if track_idx >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let deleted_engine_id = self.app.graph.track_engine_ids[track_idx];
        let deleted_rack_engine_ids = self.rack_engine_ids_for_track(track_idx);

        if self.app.is_sampler_track(track_idx) {
            self.send_sample_to_all_voices(track_idx, -1, self.app.graph.sample_rate);
        }

        for engine_id in 0..self.app.state.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                let lid_l = self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                    .load(Ordering::Relaxed);
                let lid_r = self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                    .load(Ordering::Relaxed);
                unsafe {
                    if lid_l != 0 {
                        crate::audiograph::params_push_wrapper(
                            self.app.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                idx: 0,
                                logical_id: lid_l,
                                fvalue: 0.0,
                            },
                        );
                    }
                    if lid_r != 0 {
                        crate::audiograph::params_push_wrapper(
                            self.app.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                idx: 0,
                                logical_id: lid_r,
                                fvalue: 0.0,
                            },
                        );
                    }
                }
            }
        }

        {
            let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            self.delete_custom_effect_chain(track_idx)?;
            self.delete_rack_effect_chains(track_idx, batch.serial)?;
            self.delete_track_engine_routes(track_idx);
            if let Some(track_nodes) = self.app.graph.track_node_ids.get(track_idx).cloned() {
                for (slot_idx, rack_slot) in track_nodes.rack_slots.iter().enumerate() {
                    if let (Some(engine_id), Some(route_idx)) = (
                        rack_slot.engine_id,
                        rack_slot_pool_index(track_idx, slot_idx),
                    ) {
                        self.delete_engine_route_for_track(engine_id, route_idx, track_idx);
                    }
                    self.delete_rack_slot_nodes(rack_slot);
                }
            }
            if let Some(engine_id) = deleted_engine_id {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }
            for engine_id in deleted_rack_engine_ids {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }
        }

        if !self
            .app
            .state
            .clear_track_in_place(track_idx, &self.app.graph.effect_descriptors)
        {
            return Err("Failed to clear track in place".to_string());
        }

        self.app.tracks[track_idx] = format!("Empty {}", track_idx + 1);
        self.app.sampler_paths[track_idx] = None;
        self.app.graph.track_buffer_ids[track_idx] = -1;
        self.app.graph.track_sample_rates[track_idx] = self.app.graph.sample_rate;
        self.app.graph.track_instrument_types[track_idx] = InstrumentType::Sampler;
        self.set_track_instrument_run_mode(track_idx, CustomInstrumentRunMode::Instrument)?;
        self.app.graph.track_engine_ids[track_idx] = None;
        if let Some(nodes) = self.app.graph.track_node_ids.get_mut(track_idx) {
            nodes.rack_slots.clear();
            nodes.rack_signature = None;
        }
        self.app.set_rack_selected_slot(track_idx, 0);
        self.app.graph.track_synth_node_ids[track_idx].clear();
        self.app.graph.track_gatepitch_node_ids[track_idx].clear();
        self.app.graph.effect_descriptors[track_idx] = EffectDescriptor::default_full_chain();
        self.app.graph.instrument_descriptors[track_idx] = EffectDescriptor::empty_custom_slot();
        self.app.graph.record_armed[track_idx] = false;
        self.rebind_live_track_runtime_after_delete();
        self.app.push_all_restored_defaults();
        self.app.ui.cursor_track = track_idx;
        self.app.ui.cursor_step = 0;

        Ok(track_idx)
    }

    pub fn send_sample_to_all_voices(&self, track: usize, buffer_id: i32, sample_rate: u32) {
        if track < self.app.graph.track_voice_lids.len() {
            for &lid in &self.app.graph.track_voice_lids[track] {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::sampler::PARAM_BUFFER_ID,
                            logical_id: lid,
                            fvalue: buffer_id as f32,
                        },
                    );
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::sampler::PARAM_SOURCE_SAMPLE_RATE,
                            logical_id: lid,
                            fvalue: sample_rate.max(1) as f32,
                        },
                    );
                }
            }
        }
    }

    pub fn delete_custom_effect_slot(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        if track_idx >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if slot_idx < crate::effects::BUILTIN_SLOT_COUNT {
            return Err("This effect slot cannot be deleted".to_string());
        }
        let chain_len = self.app.state.pattern.effect_chains[track_idx].len();
        if slot_idx >= chain_len {
            return Err("Invalid effect slot".to_string());
        }

        let node_id = self.app.state.pattern.effect_chains[track_idx][slot_idx]
            .node_id
            .load(Ordering::Relaxed);
        if node_id == 0 {
            return Err("Effect slot is empty".to_string());
        }
        self.app.remove_fx_slot_node(
            FxChainLocator::Track(track_idx),
            slot_idx,
            FxLeaseSlotRemoval::Shift,
        )?;

        for idx in slot_idx..chain_len.saturating_sub(1) {
            let next_idx = idx + 1;
            let next_desc = self.app.graph.effect_descriptors[track_idx][next_idx].clone();
            self.app.graph.effect_descriptors[track_idx][idx] = next_desc;
            let next_slot = &self.app.state.pattern.effect_chains[track_idx][next_idx];
            self.app.state.pattern.effect_chains[track_idx][idx].copy_from(next_slot);
        }

        if let Some(last_desc) = self.app.graph.effect_descriptors[track_idx].last_mut() {
            *last_desc = EffectDescriptor::empty_custom_slot();
        }
        if let Some(last_slot) = self.app.state.pattern.effect_chains[track_idx].last() {
            last_slot.clear();
        }
        self.app
            .state
            .remove_effect_slot_from_track_patterns(track_idx, slot_idx);

        self.app.state.publish_scheduler_snapshot();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();
        Ok(())
    }

    fn delete_custom_effect_chain(&mut self, track_idx: usize) -> Result<(), String> {
        let retire_after =
            unsafe { crate::audiograph::graph_edit_current_batch_serial(self.app.graph.lg.0) };
        if retire_after == 0 {
            return Err("Track FX chain deletion requires an edit batch".to_string());
        }
        let host = self
            .app
            .fx_chain_host(FxChainLocator::Track(track_idx))
            .ok();
        for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
            ..self.app.state.pattern.effect_chains[track_idx].len()
        {
            let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
            let node_id = slot.node_id.load(Ordering::Relaxed);
            let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, node_id as i32);
                crate::effects::conv_reverb::clear_instance(node_id as i32);
                if modulator_node_id != 0 {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_node_id as i32);
                }
            }
            self.app
                .set_track_effect_lease(track_idx, slot_idx, None, retire_after)?;
        }
        if let Some(host) = host {
            connect_fx_chain_gap(self.app.graph.lg.0, host.predecessor, host.successor);
        }
        Ok(())
    }

    fn delete_rack_effect_chains(
        &mut self,
        track_idx: usize,
        retire_after: u64,
    ) -> Result<(), String> {
        if retire_after == 0 {
            return Err("Rack FX chain deletion requires an edit batch".to_string());
        }
        let rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten();
        let Some(rack) = rack else {
            return Ok(());
        };
        for (rack_slot, slot) in rack.slots.iter().enumerate() {
            for effect in &slot.effect_slots {
                if effect.node_id == 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, effect.node_id as i32);
                    if effect.modulator_node_id != 0 {
                        crate::audiograph::delete_node(
                            self.app.graph.lg.0,
                            effect.modulator_node_id as i32,
                        );
                    }
                }
                crate::effects::conv_reverb::clear_instance(effect.node_id as i32);
            }
            self.app.editor.effect_chain_leases.retire_host(
                FxChainLocator::RackSlot {
                    track: track_idx,
                    slot: rack_slot,
                },
                retire_after,
            )?;
        }
        Ok(())
    }

    fn delete_track_engine_routes(&mut self, track_idx: usize) {
        let engine_ids = self
            .app
            .graph
            .engine_node_ids
            .iter()
            .enumerate()
            .filter_map(|(engine_id, engine)| {
                let engine = engine.as_ref()?;
                let has_audio_routes = engine
                    .route_gain_ids
                    .get(track_idx)
                    .is_some_and(|routes| !routes.is_empty());
                let has_ext_routes = engine
                    .ext_route_gain_ids
                    .get(track_idx)
                    .is_some_and(|routes| !routes.is_empty());
                (has_audio_routes || has_ext_routes).then_some(engine_id)
            })
            .collect::<Vec<_>>();
        for engine_id in engine_ids {
            self.delete_engine_route_for_track(engine_id, track_idx, track_idx);
        }
    }

    fn delete_track_shell(&mut self, track: &TrackNodeIds) {
        for rack_slot in &track.rack_slots {
            self.delete_rack_slot_nodes(rack_slot);
        }
        for &sampler_id in &track.sampler_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
            }
        }
        for &modulator_id in &track.sampler_modulator_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
            }
        }
        for &gatepitch_id in &track.sampler_gatepitch_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, gatepitch_id);
            }
        }
        unsafe {
            crate::audiograph::delete_node(self.app.graph.lg.0, track.send_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.mod_env_id);
            for &mod_in_clip_id in &track.mod_in_clip_ids {
                crate::audiograph::delete_node(self.app.graph.lg.0, mod_in_clip_id);
            }
            crate::audiograph::delete_node(self.app.graph.lg.0, track.mod_out_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.delay_id);
            if track.filter_id != 0 {
                crate::audiograph::delete_node(self.app.graph.lg.0, track.filter_id);
            }
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, track.pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_r_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_id);
        }
    }

    fn delete_rack_slot_nodes(&self, slot: &RackSlotNodeIds) {
        for &sampler_id in &slot.sampler_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
            }
        }
        for &modulator_id in &slot.sampler_modulator_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
            }
        }
        for &gatepitch_id in &slot.sampler_gatepitch_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, gatepitch_id);
            }
        }
        unsafe {
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, slot.slot_pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, slot.slot_pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, slot.slot_sum_r_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, slot.slot_sum_l_id);
        }
    }

    /// Stops an outgoing custom-instrument generation from carrying future
    /// audio produced by a shared engine. The slot panner smooths mute changes,
    /// so this closes the generation without a discontinuity while downstream
    /// rack FX remain available to render their own tails until reap.
    fn retire_custom_rack_slot_output(&self, slot: &RackSlotNodeIds) {
        push_graph_param(
            self.app.graph.lg.0,
            slot.slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            1.0,
        );
    }

    fn rack_engine_ids_for_track(&self, track_idx: usize) -> Vec<usize> {
        let Some(nodes) = self.app.graph.track_node_ids.get(track_idx) else {
            return Vec::new();
        };
        let mut engine_ids = Vec::new();
        for engine_id in nodes.rack_slots.iter().filter_map(|slot| slot.engine_id) {
            if !engine_ids.contains(&engine_id) {
                engine_ids.push(engine_id);
            }
        }
        engine_ids
    }

    fn engine_is_still_referenced(&self, engine_id: usize) -> bool {
        self.engine_is_still_referenced_excluding(engine_id, usize::MAX)
    }

    fn engine_is_still_referenced_excluding(&self, engine_id: usize, removed_track: usize) -> bool {
        self.app
            .graph
            .track_engine_ids
            .iter()
            .enumerate()
            .any(|(track_idx, binding)| track_idx != removed_track && *binding == Some(engine_id))
            || self
                .app
                .graph
                .track_node_ids
                .iter()
                .enumerate()
                .any(|(track_idx, nodes)| {
                    track_idx != removed_track
                        && nodes
                            .rack_slots
                            .iter()
                            .any(|slot| slot.engine_id == Some(engine_id))
                })
    }

    fn delete_engine_route_for_track(
        &mut self,
        engine_id: usize,
        route_idx: usize,
        track_idx: usize,
    ) {
        let track_mod_out_id = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .map(|nodes| nodes.mod_out_id);
        let Some(engine) = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(|engine| engine.as_mut())
        else {
            return;
        };
        let has_sibling_route = engine
            .route_gain_ids
            .iter()
            .enumerate()
            .any(|(idx, routes)| {
                idx != route_idx
                    && !routes.is_empty()
                    && custom_route_parent_track(idx) == Some(track_idx)
            });
        if !has_sibling_route {
            if let (Some(track_mod_out_id), Some(mod_output_channel)) = (
                track_mod_out_id,
                engine.mod_output_channels.first().copied(),
            ) {
                for &synth_id in &engine.synth_ids {
                    unsafe {
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            synth_id,
                            mod_output_channel as i32,
                            track_mod_out_id,
                            0,
                        );
                    }
                }
            }
        }
        if route_idx < engine.route_gain_ids.len() {
            for route_pair in &engine.route_gain_ids[route_idx] {
                for &route_id in route_pair {
                    if route_id > 0 {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
            engine.route_gain_ids[route_idx].clear();
        }
        if route_idx < engine.ext_route_gain_ids.len() {
            for route_ids in &engine.ext_route_gain_ids[route_idx] {
                for &route_id in route_ids {
                    if route_id > 0 {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
            engine.ext_route_gain_ids[route_idx].clear();
        }
        for voice in 0..MAX_VOICES {
            if route_idx < MAX_TRACKS {
                self.app.state.runtime.engine_route_lids[engine_id][voice][route_idx]
                    .store(0, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][route_idx]
                    .store(0, Ordering::Release);
            } else {
                self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                    .store(0, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                    .store(0, Ordering::Release);
            }
            for input in 0..EXT_MOD_INPUT_COUNT {
                if route_idx < MAX_TRACKS {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][route_idx]
                        [input]
                        .store(0, Ordering::Release);
                } else {
                    self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                        .store(0, Ordering::Release);
                }
            }
        }
        if route_idx >= MAX_TRACKS {
            self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                .store(u32::MAX, Ordering::Release);
        }
    }

    /// Removes one engine route generation from the live route tables without
    /// deleting its graph nodes. The returned concrete node ids remain valid
    /// until the deferred rack teardown reaps that generation.
    fn detach_engine_route_generation(
        &mut self,
        engine_id: usize,
        route_idx: usize,
        track_idx: usize,
    ) -> Option<DeferredEngineRouteGeneration> {
        let track_mod_out_id = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .map(|nodes| nodes.mod_out_id)?;
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(Option::as_mut)?;

        let has_sibling_route = engine
            .route_gain_ids
            .iter()
            .enumerate()
            .any(|(idx, routes)| {
                idx != route_idx
                    && !routes.is_empty()
                    && custom_route_parent_track(idx) == Some(track_idx)
            });
        if !has_sibling_route {
            if let Some(mod_output_channel) = engine.mod_output_channels.first().copied() {
                for &synth_id in &engine.synth_ids {
                    unsafe {
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            synth_id,
                            mod_output_channel as i32,
                            track_mod_out_id,
                            0,
                        );
                    }
                }
            }
        }

        let route_ids = engine
            .route_gain_ids
            .get_mut(route_idx)
            .map(std::mem::take)
            .unwrap_or_default();
        let ext_route_ids = engine
            .ext_route_gain_ids
            .get_mut(route_idx)
            .map(std::mem::take)
            .unwrap_or_default();
        let mut node_ids = route_ids
            .into_iter()
            .flatten()
            .filter(|node_id| *node_id > 0)
            .collect::<Vec<_>>();
        node_ids.extend(
            ext_route_ids
                .into_iter()
                .flatten()
                .filter(|node_id| *node_id > 0),
        );

        for voice in 0..MAX_VOICES {
            self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                .store(0, Ordering::Release);
            self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                .store(0, Ordering::Release);
            for input in 0..EXT_MOD_INPUT_COUNT {
                self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                    .store(0, Ordering::Release);
            }
        }
        self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
            .store(u32::MAX, Ordering::Release);

        (!node_ids.is_empty()).then_some(DeferredEngineRouteGeneration {
            engine_id,
            node_ids,
        })
    }

    fn enqueue_deferred_rack_teardown(&mut self, teardown: DeferredRackTeardown) {
        self.app.graph.deferred_rack_teardowns.push(teardown);
    }

    fn engine_has_deferred_route_generation(&self, engine_id: usize) -> bool {
        self.app
            .graph
            .deferred_rack_teardowns
            .iter()
            .any(|teardown| {
                teardown
                    .engine_routes
                    .iter()
                    .any(|route| route.engine_id == engine_id)
            })
    }

    fn reap_rack_teardowns(&mut self, teardowns: Vec<DeferredRackTeardown>) {
        if teardowns.is_empty() {
            return;
        }
        let mut reaped_engine_ids = Vec::new();
        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            for teardown in &teardowns {
                for route in &teardown.engine_routes {
                    for &node_id in &route.node_ids {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                        }
                    }
                    if !reaped_engine_ids.contains(&route.engine_id) {
                        reaped_engine_ids.push(route.engine_id);
                    }
                }
                for slot in &teardown.slots {
                    self.delete_rack_slot_nodes(slot);
                }
                if std::env::var_os("TINYSEQ_LOG_RACK_SYNC").is_some() {
                    eprintln!("rack sync track {}: reaped", teardown.track_idx);
                }
            }
        }
        for engine_id in reaped_engine_ids {
            if !self.engine_is_still_referenced(engine_id)
                && !self.engine_has_deferred_route_generation(engine_id)
            {
                self.delete_engine_runtime(engine_id);
            }
        }
    }

    fn reap_excess_rack_teardowns(&mut self) {
        let excess = self
            .app
            .graph
            .deferred_rack_teardowns
            .len()
            .saturating_sub(MAX_DEFERRED_RACK_TEARDOWNS);
        if excess == 0 {
            return;
        }
        let oldest = self
            .app
            .graph
            .deferred_rack_teardowns
            .drain(..excess)
            .collect();
        self.reap_rack_teardowns(oldest);
    }

    pub fn reap_due_rack_teardowns(&mut self) {
        if self.app.graph.deferred_rack_teardowns.is_empty() {
            return;
        }
        let now = Instant::now();
        let pending = std::mem::take(&mut self.app.graph.deferred_rack_teardowns);
        let (due, waiting): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|teardown| teardown.due_at <= now);
        self.app.graph.deferred_rack_teardowns = waiting;
        self.reap_rack_teardowns(due);
    }

    pub fn force_reap_all_rack_teardowns(&mut self) {
        let teardowns = std::mem::take(&mut self.app.graph.deferred_rack_teardowns);
        self.reap_rack_teardowns(teardowns);
    }

    fn rewire_engine_route_output_for_track(
        &self,
        engine_id: usize,
        track_idx: usize,
        old_sum_l_id: i32,
        old_sum_r_id: i32,
        new_sum_l_id: i32,
        new_sum_r_id: i32,
    ) -> Result<(), String> {
        let routes = self.validated_engine_route_ids_for_track(engine_id, track_idx)?;
        for [route_l_id, route_r_id] in routes {
            unsafe {
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    route_l_id,
                    0,
                    old_sum_l_id,
                    0,
                );
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    route_r_id,
                    0,
                    old_sum_r_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    route_l_id,
                    0,
                    new_sum_l_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    route_r_id,
                    0,
                    new_sum_r_id,
                    0,
                );
            }
        }
        Ok(())
    }

    fn move_engine_route_to_rack_consumer(
        &mut self,
        engine_id: usize,
        track_idx: usize,
        route_idx: usize,
    ) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(Option::as_mut)
            .ok_or_else(|| format!("Missing custom engine runtime {engine_id}"))?;
        if route_idx >= engine.route_gain_ids.len() {
            return Err(format!("Rack route consumer {route_idx} is unavailable"));
        }
        if !engine.route_gain_ids[route_idx].is_empty()
            || !engine.ext_route_gain_ids[route_idx].is_empty()
        {
            return Err(format!("Rack route consumer {route_idx} is already in use"));
        }
        engine.route_gain_ids[route_idx] = std::mem::take(&mut engine.route_gain_ids[track_idx]);
        engine.ext_route_gain_ids[route_idx] =
            std::mem::take(&mut engine.ext_route_gain_ids[track_idx]);
        self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
            .store(engine_id as u32, Ordering::Release);
        for voice in 0..MAX_VOICES {
            let [left, right] = engine.route_gain_ids[route_idx][voice];
            self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                .store(0, Ordering::Release);
            self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                .store(0, Ordering::Release);
            self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                .store(left as u64, Ordering::Release);
            self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                .store(right as u64, Ordering::Release);
            for input in 0..EXT_MOD_INPUT_COUNT {
                let ext = engine.ext_route_gain_ids[route_idx][voice][input];
                self.app.state.runtime.engine_ext_route_lids[engine_id][voice][track_idx][input]
                    .store(0, Ordering::Release);
                self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                    .store(ext as u64, Ordering::Release);
            }
        }
        Ok(())
    }

    fn validated_engine_route_ids_for_track(
        &self,
        engine_id: usize,
        track_idx: usize,
    ) -> Result<Vec<[i32; 2]>, String> {
        let routes = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .and_then(|engine| engine.route_gain_ids.get(track_idx))
            .ok_or_else(|| {
                format!(
                    "Custom engine {engine_id} has no route metadata for track {}",
                    track_idx + 1
                )
            })?;
        if routes.len() != MAX_VOICES
            || routes
                .iter()
                .any(|route_pair| route_pair[0] <= 0 || route_pair[1] <= 0)
        {
            return Err(format!(
                "Custom engine {engine_id} has an incomplete route for track {}",
                track_idx + 1
            ));
        }
        Ok(routes.clone())
    }

    fn delete_engine_runtime(&mut self, engine_id: usize) {
        let Some(engine) = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(Option::take)
        else {
            return;
        };

        for route_pairs in &engine.route_gain_ids {
            for route_pair in route_pairs {
                for &route_id in route_pair {
                    if route_id <= 0 {
                        continue;
                    }
                    unsafe {
                        crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                    }
                }
            }
        }
        for ext_routes in &engine.ext_route_gain_ids {
            for route_ids in ext_routes {
                for &route_id in route_ids {
                    if route_id > 0 {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
        }
        for &node_id in &engine.synth_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
            }
        }
        for &node_id in &engine.modulator_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
            }
        }
        for &node_id in &engine.gatepitch_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
            }
        }

        self.app.state.runtime.engine_voice_counts[engine_id].store(0, Ordering::Release);
        lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
        for voice in 0..MAX_VOICES {
            self.app.state.runtime.engine_voice_lids[engine_id][voice].store(0, Ordering::Release);
            self.app.state.runtime.engine_synth_node_ids[engine_id][voice]
                .store(0, Ordering::Release);
            self.app.state.runtime.engine_modulator_node_ids[engine_id][voice]
                .store(0, Ordering::Release);
            for track_idx in 0..MAX_TRACKS {
                self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                    .store(0, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                    .store(0, Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][track_idx]
                        [input]
                        .store(0, Ordering::Release);
                }
            }
        }
        for route_idx in MAX_TRACKS..crate::sequencer::MAX_SAMPLER_POOLS {
            if self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                .load(Ordering::Acquire)
                != engine_id as u32
            {
                continue;
            }
            self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                .store(u32::MAX, Ordering::Release);
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                    .store(0, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                    .store(0, Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                        .store(0, Ordering::Release);
                }
            }
        }
    }

    fn shift_engine_route_tables_left(&mut self, track_idx: usize, old_count: usize) {
        for engine in self.app.graph.engine_node_ids.iter_mut() {
            let Some(engine) = engine.as_mut() else {
                continue;
            };
            for idx in track_idx..old_count.saturating_sub(1) {
                engine.route_gain_ids[idx] = std::mem::take(&mut engine.route_gain_ids[idx + 1]);
                engine.ext_route_gain_ids[idx] =
                    std::mem::take(&mut engine.ext_route_gain_ids[idx + 1]);
            }
            if old_count > 0 {
                engine.route_gain_ids[old_count - 1].clear();
                engine.ext_route_gain_ids[old_count - 1].clear();
            }
            for track in track_idx..old_count.saturating_sub(1) {
                for slot in 0..MAX_RACK_SLOTS {
                    let dst = rack_slot_pool_index(track, slot).expect("valid rack route index");
                    let src = rack_slot_pool_index(track + 1, slot)
                        .expect("valid shifted rack route index");
                    engine.route_gain_ids[dst] = std::mem::take(&mut engine.route_gain_ids[src]);
                    engine.ext_route_gain_ids[dst] =
                        std::mem::take(&mut engine.ext_route_gain_ids[src]);
                }
            }
            if old_count > 0 {
                for slot in 0..MAX_RACK_SLOTS {
                    let tail = rack_slot_pool_index(old_count - 1, slot)
                        .expect("valid trailing rack route index");
                    engine.route_gain_ids[tail].clear();
                    engine.ext_route_gain_ids[tail].clear();
                }
            }
        }
        for track in track_idx..old_count.saturating_sub(1) {
            for slot in 0..MAX_RACK_SLOTS {
                let dst = rack_slot_pool_index(track, slot).expect("valid rack route index");
                let src =
                    rack_slot_pool_index(track + 1, slot).expect("valid shifted rack route index");
                let engine_id = self.app.state.runtime.rack_engine_route_engine_ids[src]
                    .load(Ordering::Acquire);
                self.app.state.runtime.rack_engine_route_engine_ids[dst]
                    .store(engine_id, Ordering::Release);
                for voice in 0..MAX_VOICES {
                    let left = self.app.state.runtime.rack_engine_route_lids[src][voice]
                        .load(Ordering::Acquire);
                    let right = self.app.state.runtime.rack_engine_route_lids_r[src][voice]
                        .load(Ordering::Acquire);
                    self.app.state.runtime.rack_engine_route_lids[dst][voice]
                        .store(left, Ordering::Release);
                    self.app.state.runtime.rack_engine_route_lids_r[dst][voice]
                        .store(right, Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        let ext = self.app.state.runtime.rack_engine_ext_route_lids[src][voice]
                            [input]
                            .load(Ordering::Acquire);
                        self.app.state.runtime.rack_engine_ext_route_lids[dst][voice][input]
                            .store(ext, Ordering::Release);
                    }
                }
            }
        }
        if old_count > 0 {
            for slot in 0..MAX_RACK_SLOTS {
                let tail = rack_slot_pool_index(old_count - 1, slot)
                    .expect("valid trailing rack route index");
                self.app.state.runtime.rack_engine_route_engine_ids[tail]
                    .store(u32::MAX, Ordering::Release);
                for voice in 0..MAX_VOICES {
                    self.app.state.runtime.rack_engine_route_lids[tail][voice]
                        .store(0, Ordering::Release);
                    self.app.state.runtime.rack_engine_route_lids_r[tail][voice]
                        .store(0, Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.app.state.runtime.rack_engine_ext_route_lids[tail][voice][input]
                            .store(0, Ordering::Release);
                    }
                }
            }
        }
    }

    fn compact_app_track_vectors(
        &mut self,
        track_idx: usize,
        retire_after: u64,
    ) -> Result<(), String> {
        let track_id = self
            .app
            .track_registry
            .id_at(track_idx)
            .ok_or_else(|| format!("Missing stable id for track {}", track_idx + 1))?;
        self.app.track_registry.remove(track_id);
        self.app.tracks.remove(track_idx);
        if track_idx < self.app.track_colors.len() {
            self.app.track_colors.remove(track_idx);
        }
        if track_idx < self.app.track_collapsed.len() {
            self.app.track_collapsed.remove(track_idx);
        }
        if track_idx < self.app.rack_selected_slots.len() {
            self.app.rack_selected_slots.remove(track_idx);
        }
        if track_idx < self.app.rack_pad_bank_starts.len() {
            self.app.rack_pad_bank_starts.remove(track_idx);
        }
        self.app.sampler_paths.remove(track_idx);
        self.app.graph.track_node_ids.remove(track_idx);
        self.app.graph.track_buffer_ids.remove(track_idx);
        self.app.graph.track_sample_rates.remove(track_idx);
        self.app.graph.track_voice_lids.remove(track_idx);
        self.app.graph.track_instrument_types.remove(track_idx);
        self.app.graph.track_instrument_run_modes.remove(track_idx);
        self.app.graph.track_engine_ids.remove(track_idx);
        self.app.graph.track_synth_node_ids.remove(track_idx);
        self.app.graph.track_gatepitch_node_ids.remove(track_idx);
        self.app.graph.effect_descriptors.remove(track_idx);
        self.app.graph.instrument_descriptors.remove(track_idx);
        self.app.graph.record_armed.remove(track_idx);
        self.app
            .editor
            .effect_chain_leases
            .retire_host(FxChainLocator::Track(track_idx), retire_after)?;
        self.app
            .editor
            .effect_chain_leases
            .reindex_tracks_after_delete(track_idx);
        Ok(())
    }

    pub fn move_appended_track_to(&mut self, target: usize) -> Result<(), String> {
        let track_count = self.app.tracks.len();
        let last = track_count.checked_sub(1)
            .ok_or_else(|| "Cannot insert into an empty track topology".to_string())?;
        if target > last {
            return Err(format!("Track insertion index {} is out of range", target + 1));
        }
        if target == last {
            return Ok(());
        }
        let track_id = self.app.track_registry.id_at(last)
            .ok_or_else(|| "Appended track has no stable identity".to_string())?;
        self.move_engine_route_tables_from_end_to(target, track_count);

        fn move_last_to<T>(values: &mut Vec<T>, target: usize) {
            let value = values.pop().expect("aligned track vector must be non-empty");
            values.insert(target, value);
        }
        move_last_to(&mut self.app.tracks, target);
        move_last_to(&mut self.app.track_colors, target);
        move_last_to(&mut self.app.track_collapsed, target);
        move_last_to(&mut self.app.rack_selected_slots, target);
        move_last_to(&mut self.app.rack_pad_bank_starts, target);
        move_last_to(&mut self.app.sampler_paths, target);
        move_last_to(&mut self.app.graph.track_node_ids, target);
        move_last_to(&mut self.app.graph.track_buffer_ids, target);
        move_last_to(&mut self.app.graph.track_sample_rates, target);
        move_last_to(&mut self.app.graph.track_voice_lids, target);
        move_last_to(&mut self.app.graph.track_instrument_types, target);
        move_last_to(&mut self.app.graph.track_instrument_run_modes, target);
        move_last_to(&mut self.app.graph.track_engine_ids, target);
        move_last_to(&mut self.app.graph.track_synth_node_ids, target);
        move_last_to(&mut self.app.graph.track_gatepitch_node_ids, target);
        move_last_to(&mut self.app.graph.effect_descriptors, target);
        move_last_to(&mut self.app.graph.instrument_descriptors, target);
        move_last_to(&mut self.app.graph.record_armed, target);
        self.app.track_registry.move_to(track_id, target)
            .map_err(|error| format!("Failed to insert stable track identity: {error:?}"))?;
        self.app.editor.effect_chain_leases.reindex_tracks_move_last_to(last, target);
        self.rebind_live_track_runtime_after_delete();
        self.rebind_all_track_graph_runtime();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();
        self.debug_assert_track_vectors_aligned();
        Ok(())
    }

    fn move_engine_route_tables_from_end_to(&mut self, target: usize, track_count: usize) {
        let last = track_count - 1;
        for engine in self.app.graph.engine_node_ids.iter_mut().filter_map(Option::as_mut) {
            let route = std::mem::take(&mut engine.route_gain_ids[last]);
            let ext = std::mem::take(&mut engine.ext_route_gain_ids[last]);
            for track in (target + 1..=last).rev() {
                engine.route_gain_ids[track] = std::mem::take(&mut engine.route_gain_ids[track - 1]);
                engine.ext_route_gain_ids[track] =
                    std::mem::take(&mut engine.ext_route_gain_ids[track - 1]);
            }
            engine.route_gain_ids[target] = route;
            engine.ext_route_gain_ids[target] = ext;
            for slot in 0..MAX_RACK_SLOTS {
                let last_pool = rack_slot_pool_index(last, slot).expect("valid rack pool");
                let route = std::mem::take(&mut engine.route_gain_ids[last_pool]);
                let ext = std::mem::take(&mut engine.ext_route_gain_ids[last_pool]);
                for track in (target + 1..=last).rev() {
                    let dst = rack_slot_pool_index(track, slot).expect("valid rack pool");
                    let src = rack_slot_pool_index(track - 1, slot).expect("valid rack pool");
                    engine.route_gain_ids[dst] = std::mem::take(&mut engine.route_gain_ids[src]);
                    engine.ext_route_gain_ids[dst] =
                        std::mem::take(&mut engine.ext_route_gain_ids[src]);
                }
                let target_pool = rack_slot_pool_index(target, slot).expect("valid rack pool");
                engine.route_gain_ids[target_pool] = route;
                engine.ext_route_gain_ids[target_pool] = ext;
            }
        }
        for engine_id in 0..self.app.state.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                let left = self.app.state.runtime.engine_route_lids[engine_id][voice][last]
                    .load(Ordering::Acquire);
                let right = self.app.state.runtime.engine_route_lids_r[engine_id][voice][last]
                    .load(Ordering::Acquire);
                let ext: [u64; EXT_MOD_INPUT_COUNT] = std::array::from_fn(|input| {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][last][input]
                        .load(Ordering::Acquire)
                });
                for track in (target + 1..=last).rev() {
                    let source = track - 1;
                    self.app.state.runtime.engine_route_lids[engine_id][voice][track].store(
                        self.app.state.runtime.engine_route_lids[engine_id][voice][source]
                            .load(Ordering::Acquire), Ordering::Release);
                    self.app.state.runtime.engine_route_lids_r[engine_id][voice][track].store(
                        self.app.state.runtime.engine_route_lids_r[engine_id][voice][source]
                            .load(Ordering::Acquire), Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.app.state.runtime.engine_ext_route_lids[engine_id][voice][track][input]
                            .store(self.app.state.runtime.engine_ext_route_lids[engine_id][voice]
                                [source][input].load(Ordering::Acquire), Ordering::Release);
                    }
                }
                self.app.state.runtime.engine_route_lids[engine_id][voice][target]
                    .store(left, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][target]
                    .store(right, Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][target][input]
                        .store(ext[input], Ordering::Release);
                }
            }
        }
        for slot in 0..MAX_RACK_SLOTS {
            let last_pool = rack_slot_pool_index(last, slot).expect("valid rack pool");
            let engine_id = self.app.state.runtime.rack_engine_route_engine_ids[last_pool]
                .load(Ordering::Acquire);
            let left: [u64; MAX_VOICES] = std::array::from_fn(|voice| {
                self.app.state.runtime.rack_engine_route_lids[last_pool][voice]
                    .load(Ordering::Acquire)
            });
            let right: [u64; MAX_VOICES] = std::array::from_fn(|voice| {
                self.app.state.runtime.rack_engine_route_lids_r[last_pool][voice]
                    .load(Ordering::Acquire)
            });
            let ext: [[u64; EXT_MOD_INPUT_COUNT]; MAX_VOICES] =
                std::array::from_fn(|voice| std::array::from_fn(|input| {
                    self.app.state.runtime.rack_engine_ext_route_lids[last_pool][voice][input]
                        .load(Ordering::Acquire)
                }));
            for track in (target + 1..=last).rev() {
                let dst = rack_slot_pool_index(track, slot).expect("valid rack pool");
                let src = rack_slot_pool_index(track - 1, slot).expect("valid rack pool");
                self.app.state.runtime.rack_engine_route_engine_ids[dst].store(
                    self.app.state.runtime.rack_engine_route_engine_ids[src]
                        .load(Ordering::Acquire), Ordering::Release);
                for voice in 0..MAX_VOICES {
                    self.app.state.runtime.rack_engine_route_lids[dst][voice].store(
                        self.app.state.runtime.rack_engine_route_lids[src][voice]
                            .load(Ordering::Acquire), Ordering::Release);
                    self.app.state.runtime.rack_engine_route_lids_r[dst][voice].store(
                        self.app.state.runtime.rack_engine_route_lids_r[src][voice]
                            .load(Ordering::Acquire), Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.app.state.runtime.rack_engine_ext_route_lids[dst][voice][input].store(
                            self.app.state.runtime.rack_engine_ext_route_lids[src][voice][input]
                                .load(Ordering::Acquire), Ordering::Release);
                    }
                }
            }
            let target_pool = rack_slot_pool_index(target, slot).expect("valid rack pool");
            self.app.state.runtime.rack_engine_route_engine_ids[target_pool]
                .store(engine_id, Ordering::Release);
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.rack_engine_route_lids[target_pool][voice]
                    .store(left[voice], Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[target_pool][voice]
                    .store(right[voice], Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.rack_engine_ext_route_lids[target_pool][voice][input]
                        .store(ext[voice][input], Ordering::Release);
                }
            }
        }
        self.rebind_rack_sampler_runtime_pools();
    }

    fn rebind_all_track_graph_runtime(&mut self) {
        for track in 0..self.app.tracks.len() {
            let nodes = &self.app.graph.track_node_ids[track];
            let voices = &self.app.graph.track_voice_lids[track];
            self.app.state.runtime.voice_counts[track]
                .store(voices.len() as u32, Ordering::Release);
            self.app.state.runtime.sampler_lids[track]
                .store(voices.first().copied().unwrap_or(0), Ordering::Release);
            self.app.state.runtime.pan_lids[track].store(nodes.pan_id as u64, Ordering::Release);
            self.app.state.runtime.delay_lids[track].store(nodes.delay_id as u64, Ordering::Release);
            self.app.state.runtime.send_lids[track].store(nodes.send_id as u64, Ordering::Release);
            self.app.state.runtime.modulator_lids[track].store(
                if self.app.graph.track_instrument_types[track] == InstrumentType::Modulator {
                    nodes.mod_env_id as u64
                } else { 0 },
                Ordering::Release,
            );
            self.app.state.runtime.instrument_type_flags[track].store(
                self.app.graph.track_instrument_types[track].runtime_flag(), Ordering::Release);
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.voice_lids[track][voice]
                    .store(voices.get(voice).copied().unwrap_or(0), Ordering::Release);
                self.app.state.runtime.synth_node_ids[track][voice].store(
                    nodes.sampler_ids.get(voice).copied().and_then(|id| u32::try_from(id).ok())
                        .unwrap_or(0), Ordering::Release);
                self.app.state.runtime.sampler_gatepitch_node_ids[track][voice].store(
                    nodes.sampler_gatepitch_ids.get(voice).copied()
                        .and_then(|id| u32::try_from(id).ok()).unwrap_or(0), Ordering::Release);
                self.app.state.runtime.sampler_modulator_node_ids[track][voice].store(
                    nodes.sampler_modulator_ids.get(voice).copied()
                        .and_then(|id| u32::try_from(id).ok()).unwrap_or(0), Ordering::Release);
            }
            self.app.publish_sampler_analysis_runtime(track);
        }
        self.rebind_rack_sampler_runtime_pools();
    }

    fn create_dedicated_engine_descriptor_from(
        &mut self,
        engine_id: usize,
    ) -> Result<usize, String> {
        if self.app.editor.engine_registry.engines.len()
            >= self.app.state.runtime.engine_voice_lids.len()
        {
            return Err(format!(
                "Instrument engine runtime slots are exhausted; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }
        let descriptor = self
            .app
            .editor
            .engine_registry
            .get(engine_id)
            .cloned()
            .ok_or_else(|| format!("Missing instrument engine descriptor {engine_id}"))?;
        let dedicated_id = self.app.editor.engine_registry.upsert(EngineDescriptor {
            name: descriptor.name,
            source: descriptor.source,
            manifest: descriptor.manifest,
            lib_index: descriptor.lib_index,
            shared_runtime: false,
        });
        if dedicated_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slots are exhausted; cannot create dedicated free-patch engine {dedicated_id}"
            ));
        }
        Ok(dedicated_id)
    }

    fn ensure_track_uses_dedicated_engine(&mut self, track: usize) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Ok(());
        }
        let old_engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        let descriptor = self
            .app
            .editor
            .engine_registry
            .get(old_engine_id)
            .cloned()
            .ok_or_else(|| format!("Missing instrument engine descriptor {old_engine_id}"))?;
        if !descriptor.shared_runtime {
            return Ok(());
        }
        if descriptor.lib_index >= self.app.editor.instrument_libs.len() {
            return Err(format!(
                "Instrument engine {old_engine_id} references missing library {}",
                descriptor.lib_index
            ));
        }

        let dedicated_engine_id = self.create_dedicated_engine_descriptor_from(old_engine_id)?;
        let manifest = descriptor.manifest;
        let name = descriptor.name;
        let lib_ptr: *const LoadedDGenLib = &self.app.editor.instrument_libs[descriptor.lib_index];
        let track_name = self.app.tracks[track].clone();
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Missing graph nodes for track {}", track + 1))?;

        self.delete_track_engine_routes(track);
        if !self.engine_is_still_referenced_excluding(old_engine_id, track) {
            self.delete_engine_runtime(old_engine_id);
        }

        self.ensure_custom_engine_runtime(dedicated_engine_id, &name, &manifest, unsafe {
            &*lib_ptr
        })?;
        self.connect_engine_to_track(
            dedicated_engine_id,
            track,
            track,
            &track_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;

        self.app.graph.track_engine_ids[track] = Some(dedicated_engine_id);
        self.app.state.runtime.track_engine_ids[track]
            .store(dedicated_engine_id as u32, Ordering::Release);
        if let Some(engine) = self.app.graph.engine_node_ids[dedicated_engine_id].as_ref() {
            self.app.graph.track_synth_node_ids[track] = engine.synth_ids.clone();
            self.app.graph.track_gatepitch_node_ids[track] = engine.gatepitch_ids.clone();
        }
        if let Some(sound) = self
            .app
            .state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            sound.engine_id = Some(dedicated_engine_id);
        }
        Ok(())
    }

    fn set_engine_voice_route_to_track(
        &self,
        engine_id: usize,
        voice_idx: usize,
        track_idx: usize,
        value: f32,
    ) {
        let Some(engine) = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
        else {
            return;
        };
        if let Some(route_pair) = engine
            .route_gain_ids
            .get(track_idx)
            .and_then(|routes| routes.get(voice_idx))
        {
            for &route_id in route_pair {
                if route_id > 0 {
                    push_graph_param(self.app.graph.lg.0, route_id as u64, 0, value);
                }
            }
        }
        if let Some(ext_routes) = engine
            .ext_route_gain_ids
            .get(track_idx)
            .and_then(|routes| routes.get(voice_idx))
        {
            for &route_id in ext_routes {
                if route_id > 0 {
                    push_graph_param(self.app.graph.lg.0, route_id as u64, 0, value);
                }
            }
        }
    }

    fn route_free_patch_idle_voice_to_track(
        &self,
        engine_id: usize,
        track: usize,
    ) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {engine_id}"))?;
        if engine.synth_ids.is_empty() || engine.gatepitch_ids.is_empty() {
            return Err(format!(
                "Instrument engine {engine_id} has no voice 0 runtime for free-patch mode"
            ));
        }
        let transport_playing = self.app.state.transport.playing.load(Ordering::Acquire);
        for track_idx in 0..self.app.tracks.len() {
            let value = free_patch_idle_route_value(track_idx, track, transport_playing);
            self.set_engine_voice_route_to_track(engine_id, 0, track_idx, value);
        }
        Ok(())
    }

    fn close_free_patch_idle_route(&self, track: usize) {
        let Some(engine_id) = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
        else {
            return;
        };
        self.set_engine_voice_route_to_track(engine_id, 0, track, 0.0);
    }

    fn dispatch_instrument_defaults_to_engine_voice(
        &self,
        track: usize,
        engine_id: usize,
        voice_idx: usize,
    ) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {engine_id}"))?;
        let synth_id =
            engine.synth_ids.get(voice_idx).copied().ok_or_else(|| {
                format!("Missing synth node for engine {engine_id} voice {voice_idx}")
            })? as u64;
        let modulator_id = engine.modulator_ids.get(voice_idx).copied().unwrap_or(0) as u64;
        let slot = &self.app.state.pattern.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        let mut param_indices = (0..num_params).collect::<Vec<_>>();
        param_indices.sort_by_key(|param_idx| slot.resolve_node_idx(*param_idx));
        for param_idx in param_indices {
            let idx = slot.resolve_node_idx(param_idx);
            let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
            let logical_id = if is_mod_param { modulator_id } else { synth_id };
            let resolved_idx = if is_mod_param {
                idx - crate::voice_modulator::MOD_PARAM_BASE as u64
            } else {
                idx
            };
            push_graph_param_span(
                self.app.graph.lg.0,
                logical_id,
                resolved_idx,
                slot.resolve_node_span(param_idx),
                slot.defaults.get(param_idx),
            );
        }
        Ok(())
    }

    fn push_free_patch_idle_gatepitch(&self, engine_id: usize) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {engine_id}"))?;
        let gatepitch_id = engine
            .gatepitch_ids
            .first()
            .copied()
            .ok_or_else(|| format!("Missing gatepitch node for engine {engine_id} voice 0"))?
            as u64;
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_TRIGGER,
            0.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_PITCH,
            440.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_VELOCITY,
            1.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_GATE,
            0.0,
        );
        Ok(())
    }

    fn apply_free_patch_idle_voice(&self, track: usize) -> Result<(), String> {
        let engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        self.route_free_patch_idle_voice_to_track(engine_id, track)?;
        self.dispatch_instrument_defaults_to_engine_voice(track, engine_id, 0)?;
        self.push_free_patch_idle_gatepitch(engine_id)?;
        lisp_host::set_dgen_engine_enabled_voices(engine_id, 1);
        Ok(())
    }

    pub fn set_track_instrument_run_mode(
        &mut self,
        track: usize,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<(), String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        let normalized_mode =
            if self.app.graph.track_instrument_types.get(track) == Some(&InstrumentType::Custom) {
                run_mode
            } else {
                CustomInstrumentRunMode::Instrument
            };

        if normalized_mode == CustomInstrumentRunMode::FreePatch {
            self.ensure_track_uses_dedicated_engine(track)?;
        }

        if let Some(mode) = self.app.graph.track_instrument_run_modes.get_mut(track) {
            *mode = normalized_mode;
        }
        self.app.state.pattern.instrument_run_modes[track]
            .store(normalized_mode.runtime_flag(), Ordering::Relaxed);
        self.app.state.runtime.instrument_run_mode_flags[track]
            .store(normalized_mode.runtime_flag(), Ordering::Release);

        self.app
            .state
            .normalize_current_pattern_instrument_run_mode(
                self.app.tracks.len(),
                &self.app.graph.effect_descriptors,
                track,
                normalized_mode,
            );
        if normalized_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)?;
        } else {
            self.close_free_patch_idle_route(track);
        }
        self.app.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub fn sync_track_instrument_run_modes_from_live_state(&mut self) -> Result<(), String> {
        let track_count = self.app.tracks.len();
        self.app
            .graph
            .track_instrument_run_modes
            .resize(track_count, CustomInstrumentRunMode::Instrument);
        for track in 0..track_count {
            let mode = CustomInstrumentRunMode::from_runtime_flag(
                self.app.state.pattern.instrument_run_modes[track].load(Ordering::Relaxed),
            );
            let mode = if self.app.graph.track_instrument_types.get(track)
                == Some(&InstrumentType::Custom)
            {
                mode
            } else {
                CustomInstrumentRunMode::Instrument
            };
            if mode == CustomInstrumentRunMode::FreePatch {
                self.ensure_track_uses_dedicated_engine(track)?;
            }
            self.app.graph.track_instrument_run_modes[track] = mode;
            self.app.state.runtime.instrument_run_mode_flags[track]
                .store(mode.runtime_flag(), Ordering::Release);
            if mode == CustomInstrumentRunMode::FreePatch {
                self.apply_free_patch_idle_voice(track)?;
            } else {
                self.close_free_patch_idle_route(track);
            }
        }
        Ok(())
    }

    fn rebind_live_track_runtime_after_delete(&mut self) {
        let mut track_sound_state = self.app.state.pattern.track_sound_state.lock().unwrap();

        for track_idx in 0..self.app.tracks.len() {
            if let (Some(nodes), Some(descs), Some(chain)) = (
                self.app.graph.track_node_ids.get(track_idx),
                self.app.graph.effect_descriptors.get(track_idx),
                self.app.state.pattern.effect_chains.get(track_idx),
            ) {
                for (slot_idx, slot) in chain.iter().enumerate() {
                    let Some(desc) = descs.get(slot_idx) else {
                        continue;
                    };
                    let node_id = slot.node_id.load(Ordering::Relaxed);
                    slot.sync_descriptor(desc, node_id);
                }
            }

            let engine_id = self
                .app
                .graph
                .track_engine_ids
                .get(track_idx)
                .and_then(|id| *id);
            self.app.state.runtime.track_engine_ids[track_idx].store(
                engine_id.map(|id| id as u32).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            let run_mode = self
                .app
                .graph
                .track_instrument_run_modes
                .get(track_idx)
                .copied()
                .unwrap_or(CustomInstrumentRunMode::Instrument);
            self.app.state.runtime.instrument_run_mode_flags[track_idx]
                .store(run_mode.runtime_flag(), Ordering::Relaxed);
            if let Some(meta) = track_sound_state.get_mut(track_idx) {
                meta.engine_id = engine_id;
            }

            if self.app.graph.track_instrument_types.get(track_idx)
                == Some(&crate::sequencer::InstrumentType::Custom)
            {
                if let Some(desc) = self.app.graph.instrument_descriptors.get(track_idx) {
                    let node_id = self.app.state.pattern.instrument_slots[track_idx]
                        .node_id
                        .load(Ordering::Relaxed);
                    self.app.state.pattern.instrument_slots[track_idx]
                        .sync_descriptor(desc, node_id);
                }
            }
        }
        drop(track_sound_state);
        self.rebind_rack_sampler_runtime_pools();
    }

    fn rebind_rack_sampler_runtime_pools(&mut self) {
        self.clear_all_rack_sampler_runtime_pools();
        for track_idx in 0..self.app.graph.track_node_ids.len() {
            self.publish_rack_slot_panner_runtime(track_idx);
            let slot_count = self.app.graph.track_node_ids[track_idx].rack_slots.len();
            for slot_idx in 0..slot_count {
                let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                    continue;
                };
                let (sampler_ids, gatepitch_ids, modulator_ids, voice_lids, has_sampler_slot) = {
                    let slot = &self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx];
                    (
                        slot.sampler_ids.clone(),
                        slot.sampler_gatepitch_ids.clone(),
                        slot.sampler_modulator_ids.clone(),
                        slot.sampler_voice_lids.clone(),
                        slot.sampler_pool_id.is_some(),
                    )
                };
                if !has_sampler_slot {
                    continue;
                }
                self.publish_sampler_voice_runtime(
                    pool_id,
                    &voice_lids,
                    &sampler_ids,
                    &gatepitch_ids,
                    &modulator_ids,
                );
                self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx].sampler_pool_id =
                    Some(pool_id);
            }
        }
    }

    fn publish_rack_slot_panner_runtime(&self, track_idx: usize) {
        let Some(track_nodes) = self.app.graph.track_node_ids.get(track_idx) else {
            return;
        };
        for slot_idx in 0..MAX_RACK_SLOTS {
            let lid = track_nodes
                .rack_slots
                .get(slot_idx)
                .map(|slot| slot.slot_pan_id as u64)
                .unwrap_or(0);
            self.app.state.runtime.rack_slot_pan_lids[track_idx][slot_idx]
                .store(lid, Ordering::Release);
        }
    }

    fn clear_rack_sampler_runtime_pools_for_track(&self, track_idx: usize) {
        for slot_idx in 0..MAX_RACK_SLOTS {
            let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                continue;
            };
            self.clear_sampler_runtime_pool(pool_id);
        }
    }

    fn validate_rack_slot_graph_rebuild(
        &self,
        track_idx: usize,
        rack: &RackTrackSnapshot,
    ) -> Result<(), String> {
        if self.app.graph.track_node_ids.get(track_idx).is_none() {
            return Err(format!("Track {} has no graph nodes", track_idx + 1));
        }
        if rack.slots.len() > MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }
        validate_rack_slot_pad_map(rack.routing, &rack.slots)?;
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            match slot.instrument_type {
                InstrumentType::Sampler => {
                    if slot.sample_id.is_none() {
                        return Err(format!(
                            "Rack sampler layer {} is missing sample metadata",
                            slot_idx + 1
                        ));
                    }
                    if rack_slot_pool_index(track_idx, slot_idx).is_none() {
                        return Err(format!(
                            "Rack sampler pool unavailable for track {track_idx} slot {slot_idx}"
                        ));
                    }
                }
                InstrumentType::Custom | InstrumentType::Modulator => {
                    let engine_id = slot.track_sound_state.engine_id.ok_or_else(|| {
                        format!(
                            "Rack instrument layer {} is missing engine metadata",
                            slot_idx + 1
                        )
                    })?;
                    let descriptor =
                        self.app
                            .editor
                            .engine_registry
                            .get(engine_id)
                            .ok_or_else(|| {
                                format!(
                                    "Rack instrument layer {} references missing engine {}",
                                    slot_idx + 1,
                                    engine_id
                                )
                            })?;
                    if descriptor.lib_index >= self.app.editor.instrument_libs.len() {
                        return Err(format!(
                            "Rack instrument layer {} references missing engine library {}",
                            slot_idx + 1,
                            descriptor.lib_index
                        ));
                    }
                }
                InstrumentType::Rack => {
                    return Err("Nested rack layers are not supported".to_string());
                }
            }
        }
        Ok(())
    }

    fn apply_rack_scene_state_in_place(
        &mut self,
        track_idx: usize,
        rack: &mut RackTrackSnapshot,
    ) -> Result<Vec<(EffectDescriptor, u32, u32)>, String> {
        let mut bindings = Vec::with_capacity(rack.slots.len());

        for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
            let nodes = self
                .app
                .graph
                .track_node_ids
                .get(track_idx)
                .and_then(|track| track.rack_slots.get(slot_idx))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Rack track {} is missing live nodes for slot {}",
                        track_idx + 1,
                        slot_idx + 1
                    )
                })?;

            let (descriptor, node_id, modulator_node_id) = match slot.instrument_type {
                InstrumentType::Sampler => {
                    let (buffer_id, _sample_name, sample_rate) =
                        slot.sample_id.as_ref().ok_or_else(|| {
                            format!(
                                "Rack sampler layer {} is missing sample metadata",
                                slot_idx + 1
                            )
                        })?;
                    for &logical_id in &nodes.sampler_voice_lids {
                        unsafe {
                            crate::audiograph::params_push_wrapper(
                                self.app.graph.lg.0,
                                crate::audiograph::ParamMsg {
                                    idx: crate::sampler::PARAM_BUFFER_ID,
                                    logical_id,
                                    fvalue: *buffer_id as f32,
                                },
                            );
                            crate::audiograph::params_push_wrapper(
                                self.app.graph.lg.0,
                                crate::audiograph::ParamMsg {
                                    idx: crate::sampler::PARAM_SOURCE_SAMPLE_RATE,
                                    logical_id,
                                    fvalue: (*sample_rate).max(1) as f32,
                                },
                            );
                        }
                    }
                    (
                        EffectDescriptor::builtin_sampler(),
                        first_graph_node_identity(&nodes.sampler_ids),
                        first_graph_node_identity(&nodes.sampler_modulator_ids),
                    )
                }
                InstrumentType::Custom | InstrumentType::Modulator => {
                    let engine_id = nodes.engine_id.ok_or_else(|| {
                        format!(
                            "Rack instrument layer {} has no live engine binding",
                            slot_idx + 1
                        )
                    })?;
                    let engine_descriptor = self
                        .app
                        .editor
                        .engine_registry
                        .get(engine_id)
                        .ok_or_else(|| {
                            format!(
                                "Rack instrument layer {} references missing engine {}",
                                slot_idx + 1,
                                engine_id
                            )
                        })?;
                    let engine = self
                        .app
                        .graph
                        .engine_node_ids
                        .get(engine_id)
                        .and_then(Option::as_ref)
                        .ok_or_else(|| {
                            format!(
                                "Rack instrument layer {} is missing live engine runtime {}",
                                slot_idx + 1,
                                engine_id
                            )
                        })?;
                    (
                        lisp_host::instrument_descriptor_from_manifest(
                            &engine_descriptor.name,
                            &engine_descriptor.manifest,
                        ),
                        first_graph_node_identity(&engine.synth_ids),
                        first_graph_node_identity(&engine.modulator_ids),
                    )
                }
                InstrumentType::Rack => {
                    return Err("Nested rack layers are not supported".to_string());
                }
            };

            slot.instrument_slot.sync_to_descriptor_with_modulator(
                &descriptor,
                node_id,
                modulator_node_id,
            );
            bindings.push((descriptor, node_id, modulator_node_id));

            for (param, value) in [
                (
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    slot.gain,
                ),
                (
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    slot.pan,
                ),
                (
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                    if slot.mute { 1.0 } else { 0.0 },
                ),
            ] {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: param,
                            logical_id: nodes.slot_pan_id as u64,
                            fvalue: value,
                        },
                    );
                }
            }
        }

        let has_solo = rack.slots.iter().any(|slot| slot.solo);
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            let nodes = &self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx];
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.app.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                        logical_id: nodes.slot_pan_id as u64,
                        fvalue: if has_solo && !slot.solo { 1.0 } else { 0.0 },
                    },
                );
            }
        }
        self.publish_rack_slot_panner_runtime(track_idx);

        Ok(bindings)
    }

    fn rebuild_rack_slot_graph(
        &mut self,
        track_idx: usize,
        rack: &mut RackTrackSnapshot,
    ) -> Result<Vec<(EffectDescriptor, u32, u32)>, String> {
        self.validate_rack_slot_graph_rebuild(track_idx, rack)?;
        let old_engine_ids = self.rack_engine_ids_for_track(track_idx);
        let track_nodes = self.app.graph.track_node_ids[track_idx].clone();
        let has_solo = rack.slots.iter().any(|slot| slot.solo);
        let mut rebuilt_nodes = Vec::with_capacity(rack.slots.len());
        let mut bindings = Vec::with_capacity(rack.slots.len());

        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            self.retire_rack_slot_graph_generation(track_idx);

            for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
                let slot_name = format!("{}_rack{}", self.app.tracks[track_idx], slot_idx + 1);
                let mixer = self.create_rack_slot_mixer(
                    &slot_name,
                    track_nodes.voice_sum_id,
                    track_nodes.voice_sum_r_id,
                    slot.gain,
                    slot.pan,
                    slot.mute,
                    has_solo && !slot.solo,
                )?;

                match slot.instrument_type {
                    InstrumentType::Sampler => {
                        let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                            return Err(format!(
                                "Rack sampler pool unavailable for track {track_idx} slot {slot_idx}"
                            ));
                        };
                        let (buffer_id, _sample_name, sample_rate) =
                            slot.sample_id.clone().ok_or_else(|| {
                                format!(
                                    "Rack sampler layer {} is missing sample metadata",
                                    slot_idx + 1
                                )
                            })?;
                        let voices = self.build_sampler_voices(
                            pool_id,
                            &slot_name,
                            buffer_id,
                            sample_rate,
                            mixer.slot_sum_l_id,
                            mixer.slot_sum_r_id,
                            track_nodes.mod_in_clip_ids,
                            slot.max_polyphony,
                        )?;
                        self.publish_sampler_voice_runtime(
                            pool_id,
                            &voices.voice_lids,
                            &voices.sampler_ids,
                            &voices.gatepitch_ids,
                            &voices.modulator_ids,
                        );
                        let descriptor = EffectDescriptor::builtin_sampler();
                        let node_id = first_graph_node_identity(&voices.sampler_ids);
                        let modulator_node_id = first_graph_node_identity(&voices.modulator_ids);
                        slot.instrument_slot.sync_to_descriptor_with_modulator(
                            &descriptor,
                            node_id,
                            modulator_node_id,
                        );
                        bindings.push((descriptor, node_id, modulator_node_id));
                        rebuilt_nodes.push(RackSlotNodeIds {
                            sampler_pool_id: Some(pool_id),
                            engine_id: None,
                            sampler_voice_lids: voices.voice_lids,
                            sampler_ids: voices.sampler_ids,
                            sampler_gatepitch_ids: voices.gatepitch_ids,
                            sampler_modulator_ids: voices.modulator_ids,
                            slot_sum_l_id: mixer.slot_sum_l_id,
                            slot_sum_r_id: mixer.slot_sum_r_id,
                            slot_pan_id: mixer.slot_pan_id,
                        });
                    }
                    InstrumentType::Custom | InstrumentType::Modulator => {
                        let engine_id = slot.track_sound_state.engine_id.ok_or_else(|| {
                            format!(
                                "Rack instrument layer {} is missing engine metadata",
                                slot_idx + 1
                            )
                        })?;
                        let engine_descriptor = self
                            .app
                            .editor
                            .engine_registry
                            .get(engine_id)
                            .cloned()
                            .ok_or_else(|| {
                                format!(
                                    "Rack instrument layer {} references missing engine {}",
                                    slot_idx + 1,
                                    engine_id
                                )
                            })?;
                        if self
                            .app
                            .graph
                            .engine_node_ids
                            .get(engine_id)
                            .and_then(|engine| engine.as_ref())
                            .is_none()
                        {
                            let lib_index = engine_descriptor.lib_index;
                            let lib_ptr: *const LoadedDGenLib =
                                &self.app.editor.instrument_libs[lib_index];
                            unsafe {
                                self.ensure_custom_engine_runtime(
                                    engine_id,
                                    &engine_descriptor.name,
                                    &engine_descriptor.manifest,
                                    &*lib_ptr,
                                )?;
                            }
                        }
                        self.connect_engine_to_track(
                            engine_id,
                            rack_slot_pool_index(track_idx, slot_idx).ok_or_else(|| {
                                format!("Rack slot {} has no route-consumer identity", slot_idx + 1)
                            })?,
                            track_idx,
                            &slot_name,
                            mixer.slot_sum_l_id,
                            mixer.slot_sum_r_id,
                            track_nodes.mod_out_id,
                            track_nodes.mod_in_clip_ids,
                        )?;
                        let engine = self.app.graph.engine_node_ids[engine_id]
                            .as_ref()
                            .ok_or_else(|| {
                                format!(
                                    "Rack instrument layer '{}' failed to initialize engine {}",
                                    engine_descriptor.name, engine_id
                                )
                            })?;
                        let descriptor = lisp_host::instrument_descriptor_from_manifest(
                            &engine_descriptor.name,
                            &engine_descriptor.manifest,
                        );
                        let node_id = first_graph_node_identity(&engine.synth_ids);
                        let modulator_node_id = first_graph_node_identity(&engine.modulator_ids);
                        slot.instrument_slot.sync_to_descriptor_with_modulator(
                            &descriptor,
                            node_id,
                            modulator_node_id,
                        );
                        bindings.push((descriptor, node_id, modulator_node_id));
                        rebuilt_nodes.push(RackSlotNodeIds {
                            sampler_pool_id: None,
                            engine_id: Some(engine_id),
                            sampler_voice_lids: Vec::new(),
                            sampler_ids: Vec::new(),
                            sampler_gatepitch_ids: Vec::new(),
                            sampler_modulator_ids: Vec::new(),
                            slot_sum_l_id: mixer.slot_sum_l_id,
                            slot_sum_r_id: mixer.slot_sum_r_id,
                            slot_pan_id: mixer.slot_pan_id,
                        });
                    }
                    InstrumentType::Rack => {
                        return Err("Nested rack layers are not supported".to_string());
                    }
                }
            }
            self.app.graph.track_node_ids[track_idx].rack_slots = rebuilt_nodes;
            for (slot_idx, slot) in rack.slots.iter().enumerate() {
                let nodes = &self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx];
                let host = FxChainHost {
                    locator: FxChainLocator::RackSlot {
                        track: track_idx,
                        slot: slot_idx,
                    },
                    label: format!("Track {} Rack Slot {}", track_idx + 1, slot_idx + 1),
                    predecessor: StereoEndpoint {
                        node_id: nodes.slot_pan_id,
                        channels: 2,
                    },
                    successor: ChainSuccessor::MonoPair {
                        left: track_nodes.voice_sum_id,
                        right: track_nodes.voice_sum_r_id,
                    },
                    slots: slot
                        .effect_slots
                        .iter()
                        .zip(&slot.effect_descriptors)
                        .map(|(effect, descriptor)| FxChainSlotView {
                            node_id: effect.node_id as i32,
                            modulator_node_id: effect.modulator_node_id as i32,
                            input_channels: descriptor.input_channels,
                            output_channels: descriptor.output_channels,
                        })
                        .collect(),
                };
                connect_fx_chain_host(self.app.graph.lg.0, &host);
            }
            self.publish_rack_slot_panner_runtime(track_idx);
        }

        self.app.graph.track_node_ids[track_idx].rack_signature =
            Some(rack_topology_signature(rack));

        for engine_id in old_engine_ids.iter().copied() {
            if !self.engine_is_still_referenced(engine_id) {
                // A deferred custom generation has no scheduler-visible work
                // left. Keep its graph nodes allocated for safe reap, but stop
                // running an otherwise-idle instrument voice in the meantime.
                lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
            }
        }
        self.reap_excess_rack_teardowns();
        for engine_id in old_engine_ids {
            if !self.engine_is_still_referenced(engine_id)
                && !self.engine_has_deferred_route_generation(engine_id)
            {
                self.delete_engine_runtime(engine_id);
            }
        }

        Ok(bindings)
    }

    fn retire_rack_slot_graph_generation(&mut self, track_idx: usize) {
        let old_rack_slots = self.app.graph.track_node_ids[track_idx].rack_slots.clone();
        let old_engine_routes = old_rack_slots
            .iter()
            .enumerate()
            .filter_map(|(slot_idx, slot)| {
                let engine_id = slot.engine_id?;
                self.retire_custom_rack_slot_output(slot);
                let route_idx = rack_slot_pool_index(track_idx, slot_idx)?;
                self.detach_engine_route_generation(engine_id, route_idx, track_idx)
            })
            .collect::<Vec<_>>();
        if !old_rack_slots.is_empty() || !old_engine_routes.is_empty() {
            self.enqueue_deferred_rack_teardown(DeferredRackTeardown {
                slots: old_rack_slots,
                engine_routes: old_engine_routes,
                track_idx,
                due_at: Instant::now() + RACK_TEARDOWN_TAIL,
            });
        }
        self.clear_rack_sampler_runtime_pools_for_track(track_idx);
    }

    fn create_track_shell(&mut self, idx: usize, name: &str) -> Result<TrackShell, String> {
        if !self.app.track_registry.can_allocate() {
            return Err("Stable track id space is exhausted".to_string());
        }
        let voice_sum_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{}_sum_l", name),
            "create_track_shell left voice sum",
        )?;
        let voice_sum_r_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{}_sum_r", name),
            "create_track_shell right voice sum",
        )?;

        let pan_name = CString::new(format!("{}_pan", name)).unwrap();
        let pan_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                pan_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, pan_id);
        }

        let fx_out_name = CString::new(format!("{}_fx_out", name)).unwrap();
        let fx_out_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                fx_out_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };

        let send_id = add_gain_node_checked(
            self.app.graph.lg.0,
            0.0,
            &format!("{}_send", name),
            "create_track_shell send",
        )?;
        let mod_out_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{}_mod_out", name),
            "create_track_shell mod output",
        )?;
        let mod_in_clip_ids = std::array::from_fn(|input| {
            let mod_in_name = CString::new(format!("{}_mod_in{}_clip", name, input + 1)).unwrap();
            unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::track_modulator::mod_in_clip_vtable(),
                    crate::track_modulator::MOD_IN_CLIP_STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_in_name.as_ptr(),
                    1,
                    1,
                    std::ptr::null(),
                    0,
                )
            }
        });
        let mod_env_name = CString::new(format!("{}_mod_env", name)).unwrap();
        let mod_env_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::track_modulator::modulator_envelope_vtable(),
                crate::track_modulator::MODULATOR_ENVELOPE_STATE_SIZE * std::mem::size_of::<f32>(),
                mod_env_name.as_ptr(),
                0,
                1,
                std::ptr::null(),
                0,
            )
        };

        unsafe {
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_id, 0, pan_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_r_id, 0, pan_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 0, fx_out_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 1, fx_out_id, 1);
        }
        let output = self.app.state.pattern.track_params[idx].output();
        self.connect_delay_output_to(fx_out_id, &output);

        Ok(TrackShell {
            voice_sum_id,
            voice_sum_r_id,
            pan_id,
            filter_id: 0,
            delay_id: fx_out_id,
            send_id,
            mod_out_id,
            mod_in_clip_ids,
            mod_env_id,
        })
    }

    fn create_rack_slot_mixer(
        &mut self,
        slot_name: &str,
        track_voice_sum_id: i32,
        track_voice_sum_r_id: i32,
        gain: f32,
        pan: f32,
        mute: bool,
        muted_by_solo: bool,
    ) -> Result<RackSlotMixer, String> {
        let slot_sum_l_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{slot_name}_sum_l"),
            "create_rack_slot_mixer left sum",
        )?;
        let slot_sum_r_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{slot_name}_sum_r"),
            "create_rack_slot_mixer right sum",
        )?;
        let pan_name = CString::new(format!("{slot_name}_pan")).unwrap();
        let slot_pan_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                pan_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        if slot_pan_id < 0 {
            return Err(format!(
                "create_rack_slot_mixer: failed to add panner for {slot_name}"
            ));
        }
        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, slot_pan_id);
            crate::audiograph::graph_connect(self.app.graph.lg.0, slot_sum_l_id, 0, slot_pan_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, slot_sum_r_id, 0, slot_pan_id, 1);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                slot_pan_id,
                0,
                track_voice_sum_id,
                0,
            );
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                slot_pan_id,
                1,
                track_voice_sum_r_id,
                0,
            );
        }
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
            gain.max(0.0),
        );
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
            pan.clamp(-1.0, 1.0),
        );
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            if mute { 1.0 } else { 0.0 },
        );
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            if muted_by_solo { 1.0 } else { 0.0 },
        );
        Ok(RackSlotMixer {
            slot_sum_l_id,
            slot_sum_r_id,
            slot_pan_id,
        })
    }

    fn rack_slot_append_target(
        &self,
        track_idx: usize,
    ) -> Result<(RackTrackSnapshot, usize), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        let slot_idx = rack.slots.len();
        if slot_idx >= MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }
        Ok((rack, slot_idx))
    }

    pub(super) fn refresh_rack_signature_from_live_state(&mut self, track_idx: usize) {
        let signature = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .and_then(Option::as_ref)
            .map(rack_topology_signature);
        if let Some(track_nodes) = self.app.graph.track_node_ids.get_mut(track_idx) {
            track_nodes.rack_signature = signature;
        }
    }

    fn finish_rack_track_registration(
        &mut self,
        idx: usize,
        track_name: String,
        shell: TrackShell,
        rack_slots: Vec<RackSlotNodeIds>,
        rack_track: RackTrackSnapshot,
    ) -> Result<(), String> {
        self.app
            .track_registry
            .allocate()
            .map_err(|error| format!("Failed to allocate stable track id: {error:?}"))?;
        let rack_signature = rack_topology_signature(&rack_track);
        self.app.state.runtime.voice_counts[idx].store(0, Ordering::Release);
        self.app.state.runtime.sampler_lids[idx].store(0, Ordering::Release);
        self.app.state.runtime.modulator_lids[idx].store(0, Ordering::Release);
        self.app.state.runtime.pan_lids[idx].store(shell.pan_id as u64, Ordering::Release);
        self.app.state.runtime.delay_lids[idx].store(shell.delay_id as u64, Ordering::Release);
        self.app.state.runtime.send_lids[idx].store(shell.send_id as u64, Ordering::Release);
        self.app.state.runtime.instrument_type_flags[idx]
            .store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
        self.app.state.pattern.instrument_run_modes[idx].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.app.state.runtime.instrument_run_mode_flags[idx].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.app.state.runtime.track_engine_ids[idx].store(u32::MAX, Ordering::Release);
        if let Some(sound) = self
            .app
            .state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(idx)
        {
            sound.engine_id = None;
        }

        self.app.tracks.push(track_name.clone());
        self.app.push_next_track_color();
        self.app.push_default_track_collapsed();
        self.app.rack_selected_slots.push(0);
        self.app.rack_pad_bank_starts.push(DRUM_RACK_FIRST_PAD_NOTE);
        self.app
            .graph
            .effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.app.graph.record_armed.push(false);
        self.app.graph.track_voice_lids.push(Vec::new());
        self.app
            .graph
            .track_instrument_types
            .push(InstrumentType::Rack);
        self.app
            .graph
            .track_instrument_run_modes
            .push(CustomInstrumentRunMode::Instrument);
        self.app.graph.track_buffer_ids.push(-1);
        self.app
            .graph
            .track_sample_rates
            .push(self.app.graph.sample_rate);
        self.app.graph.track_node_ids.push(TrackNodeIds {
            sampler_ids: Vec::new(),
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: shell.voice_sum_id,
            voice_sum_r_id: shell.voice_sum_r_id,
            pan_id: shell.pan_id,
            filter_id: shell.filter_id,
            delay_id: shell.delay_id,
            send_id: shell.send_id,
            mod_out_id: shell.mod_out_id,
            mod_in_clip_ids: shell.mod_in_clip_ids,
            mod_env_id: shell.mod_env_id,
            bus_send_ids: Vec::new(),
            rack_slots,
            rack_signature: Some(rack_signature),
        });
        self.publish_rack_slot_panner_runtime(idx);
        self.app.graph.track_synth_node_ids.push(Vec::new());
        self.app.graph.track_gatepitch_node_ids.push(Vec::new());
        self.app.graph.track_engine_ids.push(None);
        self.app.state.pattern.instrument_slots[idx].clear();
        self.app
            .graph
            .instrument_descriptors
            .push(EffectDescriptor::empty_custom_slot());

        self.app.state.extend_all_pattern_snapshots_to_track(
            idx + 1,
            &self.app.graph.effect_descriptors,
            idx,
            CustomInstrumentRunMode::Instrument,
            None,
        );
        self.app
            .state
            .set_rack_track_for_all_pattern_snapshots(idx, rack_track);
        self.app.refresh_effect_sidechain_labels();

        self.app
            .state
            .transport
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);
        self.app.ui.cursor_track = idx;
        self.app.ui.cursor_step = 0;
        self.app.ui.focused_region = super::Region::Cirklon;
        self.app.ui.sidebar_tab = super::SidebarTab::Tools;
        self.app.ui.sidebar_mode = super::SidebarMode::Audition;
        self.app.ui.sidebar_search_focused = false;
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn build_sampler_voices(
        &mut self,
        sampler_pool_id: usize,
        track_name: &str,
        buffer_id: i32,
        sample_rate: u32,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
        voice_count: usize,
    ) -> Result<SamplerVoiceSetup, String> {
        if sampler_pool_id >= MAX_SAMPLER_POOLS {
            return Err(format!(
                "Sampler pool {sampler_pool_id} is unavailable; maximum sampler pools is {MAX_SAMPLER_POOLS}"
            ));
        }
        let voice_count = voice_count.clamp(1, MAX_VOICES);
        let mut sampler_ids = Vec::with_capacity(voice_count);
        let mut gatepitch_ids = Vec::with_capacity(voice_count);
        let mut modulator_ids = Vec::with_capacity(voice_count);
        let mut voice_lids = Vec::with_capacity(voice_count);
        let (node_capacity, connection_capacity) = sampler_voice_build_capacities(voice_count);
        let mut transaction = GraphNodeBuildTransaction::new(
            self.app.graph.lg.0,
            node_capacity,
            connection_capacity,
        )?;

        for v in 0..voice_count {
            let gp_name = CString::new(format!("{}_gp_{}", track_name, v)).unwrap();
            check_test_graph_build_node_add(&format!(
                "build_sampler_voices pool {sampler_pool_id} voice {v} gatepitch"
            ))?;
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::effects::gatepitch::gatepitch_vtable(),
                    crate::effects::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    crate::effects::gatepitch::OUTPUT_COUNT as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!(
                    "build_sampler_voices: failed to add gatepitch node for voice {v}"
                ));
            }
            let gp_id = transaction.own(gp_id)?;

            let mod_name = CString::new(format!("{}_mod_{}", track_name, v)).unwrap();
            let mod_initial_state =
                crate::voice_modulator::sampler_voice_initial_state(sampler_pool_id, v);
            check_test_graph_build_node_add(&format!(
                "build_sampler_voices pool {sampler_pool_id} voice {v} modulator"
            ))?;
            let mod_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::voice_modulator::voice_modulator_vtable(),
                    crate::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_name.as_ptr(),
                    crate::voice_modulator::INPUT_COUNT as i32,
                    crate::voice_modulator::NUM_OUTPUTS as i32,
                    (&mod_initial_state
                        as *const crate::voice_modulator::VoiceModulatorInitialState)
                        .cast(),
                    std::mem::size_of::<crate::voice_modulator::VoiceModulatorInitialState>(),
                )
            };
            if mod_id < 0 {
                return Err(format!(
                    "build_sampler_voices: failed to add modulator node for voice {v}"
                ));
            }
            let mod_id = transaction.own(mod_id)?;
            let node_name = format!("{}_{}", track_name, v);
            check_test_graph_build_node_add(&format!(
                "build_sampler_voices pool {sampler_pool_id} voice {v} sampler"
            ))?;
            let st = crate::sampler::create_sampler_node(
                self.app.graph.lg.0,
                buffer_id,
                sample_rate,
                &node_name,
            )?;
            let sampler_id = transaction.own(st.node_id)?;
            for port in 0..4 {
                transaction.connect(
                    gp_id,
                    port,
                    mod_id,
                    port,
                    &format!("build_sampler_voices voice {v} gatepitch port {port}"),
                )?;
            }
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_PHASE as i32,
                mod_id,
                crate::voice_modulator::INPUT_TRANSPORT_BAR_PHASE as i32,
                &format!("build_sampler_voices voice {v} transport clock"),
            )?;
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_INC as i32,
                mod_id,
                crate::voice_modulator::INPUT_TRANSPORT_BAR_PHASE_INC as i32,
                &format!("build_sampler_voices voice {v} transport clock increment"),
            )?;
            for port in 0..crate::voice_modulator::NUM_OUTPUTS {
                transaction.connect(
                    mod_id,
                    port as i32,
                    sampler_id,
                    port as i32,
                    &format!("build_sampler_voices voice {v} modulator port {port}"),
                )?;
            }
            for (input, &clip_id) in track_mod_in_clip_ids.iter().enumerate() {
                transaction.connect(
                    clip_id,
                    0,
                    mod_id,
                    (4 + input) as i32,
                    &format!("build_sampler_voices voice {v} external mod input {input}"),
                )?;
            }
            transaction.connect(
                sampler_id,
                0,
                voice_sum_id,
                0,
                &format!("build_sampler_voices voice {v} left output"),
            )?;
            transaction.connect(
                sampler_id,
                1,
                voice_sum_r_id,
                0,
                &format!("build_sampler_voices voice {v} right output"),
            )?;
            sampler_ids.push(sampler_id);
            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            voice_lids.push(st.logical_id);
        }

        transaction.commit();
        let bpm = self.app.state.transport.bpm.load(Ordering::Relaxed) as f32;
        for &mod_id in &modulator_ids {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.app.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::voice_modulator::PARAM_BPM as u64,
                        logical_id: mod_id as u64,
                        fvalue: bpm,
                    },
                );
            }
        }

        Ok(SamplerVoiceSetup {
            sampler_ids,
            gatepitch_ids,
            modulator_ids,
            voice_lids,
        })
    }

    fn publish_sampler_voice_runtime(
        &self,
        pool_id: usize,
        voice_lids: &[u64],
        sampler_ids: &[i32],
        gatepitch_ids: &[i32],
        modulator_ids: &[i32],
    ) {
        if pool_id >= self.app.state.runtime.voice_lids.len() {
            return;
        }
        self.clear_sampler_runtime_pool(pool_id);
        let voice_count = voice_lids
            .len()
            .min(sampler_ids.len())
            .min(gatepitch_ids.len())
            .min(modulator_ids.len())
            .min(MAX_VOICES);
        for v in 0..voice_count {
            self.app.state.runtime.voice_lids[pool_id][v].store(voice_lids[v], Ordering::Release);
            self.app.state.runtime.synth_node_ids[pool_id][v]
                .store(sampler_ids[v] as u32, Ordering::Release);
            self.app.state.runtime.sampler_gatepitch_node_ids[pool_id][v]
                .store(gatepitch_ids[v] as u32, Ordering::Release);
            self.app.state.runtime.sampler_modulator_node_ids[pool_id][v]
                .store(modulator_ids[v] as u32, Ordering::Release);
        }
        self.app.state.runtime.voice_counts[pool_id].store(voice_count as u32, Ordering::Release);
        self.app.state.runtime.sampler_lids[pool_id]
            .store(voice_lids.first().copied().unwrap_or(0), Ordering::Release);
    }

    fn clear_sampler_runtime_pool(&self, pool_id: usize) {
        if pool_id >= self.app.state.runtime.voice_lids.len() {
            return;
        }
        self.app.state.runtime.voice_counts[pool_id].store(0, Ordering::Release);
        self.app.state.runtime.sampler_lids[pool_id].store(0, Ordering::Release);
        for v in 0..MAX_VOICES {
            self.app.state.runtime.voice_lids[pool_id][v].store(0, Ordering::Release);
            self.app.state.runtime.synth_node_ids[pool_id][v].store(0, Ordering::Release);
            self.app.state.runtime.sampler_gatepitch_node_ids[pool_id][v]
                .store(0, Ordering::Release);
            self.app.state.runtime.sampler_modulator_node_ids[pool_id][v]
                .store(0, Ordering::Release);
        }
    }

    fn clear_all_rack_sampler_runtime_pools(&self) {
        for pool_id in MAX_TRACKS..self.app.state.runtime.voice_lids.len() {
            self.clear_sampler_runtime_pool(pool_id);
        }
    }

    fn ensure_engine_slot(&mut self, engine_id: usize) {
        while self.app.graph.engine_node_ids.len() <= engine_id {
            self.app.graph.engine_node_ids.push(None);
        }
    }

    fn graph_connect_checked(
        &self,
        src_node: i32,
        src_port: i32,
        dst_node: i32,
        dst_port: i32,
        context: &str,
    ) -> Result<(), String> {
        let ok = unsafe {
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                src_node,
                src_port,
                dst_node,
                dst_port,
            )
        };
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{context}: graph_connect({}, {}, {}, {}) failed",
                src_node, src_port, dst_node, dst_port
            ))
        }
    }

    fn connect_custom_host_inputs(
        &self,
        gp_id: i32,
        mod_id: i32,
        synth_id: i32,
        routes: &[CustomHostInputRoute],
        context: &str,
    ) -> Result<(), String> {
        for route in routes {
            let (source_node, source_port) = match route.source {
                CustomHostInputSource::GatePitch(port) => (gp_id, port),
                CustomHostInputSource::Modulator(port) => (mod_id, port),
            };
            self.graph_connect_checked(
                source_node,
                source_port,
                synth_id,
                route.input_channel,
                &format!("{context} host channel {}", route.input_channel),
            )?;
        }
        Ok(())
    }

    fn ensure_custom_engine_runtime(
        &mut self,
        engine_id: usize,
        name: &str,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        if engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }
        if self.app.graph.engine_node_ids[engine_id].is_some() {
            return Ok(());
        }

        let context = format!("ensure_custom_engine_runtime engine {engine_id}");
        let host_routes = custom_host_input_routes(manifest, &context)?;
        let mut transaction = GraphNodeBuildTransaction::new(
            self.app.graph.lg.0,
            MAX_VOICES * 3,
            MAX_VOICES * (host_routes.len() + 6),
        )?;
        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut synth_ids = Vec::with_capacity(MAX_VOICES);
        let mut modulator_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let voice_context = format!("{context} voice {v}");
            let gp_name = CString::new(format!("{}_gp_{}", name, v))
                .map_err(|_| format!("{voice_context}: gatepitch node name contains NUL"))?;
            check_test_graph_build_node_add(&format!("{voice_context} gatepitch"))?;
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::effects::gatepitch::gatepitch_vtable(),
                    crate::effects::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    crate::effects::gatepitch::OUTPUT_COUNT as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!("{voice_context}: failed to add gatepitch node"));
            }
            let gp_id = transaction.own(gp_id)?;

            let mod_name = CString::new(format!("{}_mod_{}", name, v))
                .map_err(|_| format!("{voice_context}: modulator node name contains NUL"))?;
            let mod_initial_state =
                crate::voice_modulator::custom_engine_initial_state(engine_id, v);
            check_test_graph_build_node_add(&format!("{voice_context} modulator"))?;
            let mod_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::voice_modulator::voice_modulator_vtable(),
                    crate::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_name.as_ptr(),
                    crate::voice_modulator::INPUT_COUNT as i32,
                    crate::voice_modulator::NUM_OUTPUTS as i32,
                    (&mod_initial_state
                        as *const crate::voice_modulator::VoiceModulatorInitialState)
                        .cast(),
                    std::mem::size_of::<crate::voice_modulator::VoiceModulatorInitialState>(),
                )
            };
            if mod_id < 0 {
                return Err(format!("{voice_context}: failed to add modulator node"));
            }
            let mod_id = transaction.own(mod_id)?;

            let slot_id = engine_id * MAX_VOICES + v;
            let init_msg = lisp_host::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_host::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            let synth_name = CString::new(format!("{}_engine_synth_{}", name, v))
                .map_err(|_| format!("{voice_context}: synth node name contains NUL"))?;
            check_test_graph_build_node_add(&format!("{voice_context} synth"))?;
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_host::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    manifest.n_outputs.max(1) as i32,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };
            if synth_id < 0 {
                return Err(format!(
                    "{voice_context}: failed to add synth node (manifest.n_inputs={})",
                    manifest.n_inputs
                ));
            }
            let synth_id = transaction.own(synth_id)?;
            for route in &host_routes {
                let (source_node, source_port) = match route.source {
                    CustomHostInputSource::GatePitch(port) => (gp_id, port),
                    CustomHostInputSource::Modulator(port) => (mod_id, port),
                };
                transaction.connect(
                    source_node,
                    source_port,
                    synth_id,
                    route.input_channel,
                    &format!("{voice_context} host channel {}", route.input_channel),
                )?;
            }
            for port in 0..4 {
                transaction.connect(gp_id, port, mod_id, port, &voice_context)?;
            }
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_PHASE as i32,
                mod_id,
                crate::voice_modulator::INPUT_TRANSPORT_BAR_PHASE as i32,
                &format!("{voice_context} transport clock"),
            )?;
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_INC as i32,
                mod_id,
                crate::voice_modulator::INPUT_TRANSPORT_BAR_PHASE_INC as i32,
                &format!("{voice_context} transport clock increment"),
            )?;
            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            synth_ids.push(synth_id);
            voice_lids.push(gp_id as u64);
        }

        // The process registry must be ready before the batch can reach the
        // audio thread. No fallible operations remain after this publication.
        for v in 0..MAX_VOICES {
            let slot_id = engine_id * MAX_VOICES + v;
            lisp_host::set_dgen_instrument_fn(slot_id, lib.process_fn);
            lisp_host::set_dgen_instrument_output_count(slot_id, manifest.n_outputs.max(1));
        }
        transaction.commit();

        let bpm = self.app.state.transport.bpm.load(Ordering::Relaxed) as f32;
        for &mod_id in &modulator_ids {
            push_graph_param(
                self.app.graph.lg.0,
                mod_id as u64,
                crate::voice_modulator::PARAM_BPM as u64,
                bpm,
            );
        }
        let audio_output_channels = manifest_audio_output_channels(manifest);
        let mod_output_channels = manifest_mod_output_channels(manifest);
        self.app.graph.engine_node_ids[engine_id] = Some(EngineNodeIds {
            synth_ids,
            synth_inputs: manifest.n_inputs,
            synth_outputs: audio_output_channels.len(),
            audio_output_channels,
            mod_output_channels,
            gatepitch_ids,
            modulator_ids,
            route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
            ext_route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
        });

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.runtime.engine_voice_lids[engine_id][v].store(lid, Ordering::Release);
        }
        self.app.state.runtime.engine_voice_counts[engine_id]
            .store(MAX_VOICES as u32, Ordering::Release);
        lisp_host::reset_dgen_engine_enabled_voices(engine_id);
        if let Some(engine) = &self.app.graph.engine_node_ids[engine_id] {
            for (v, &sid) in engine.synth_ids.iter().enumerate() {
                self.app.state.runtime.engine_synth_node_ids[engine_id][v]
                    .store(sid as u32, Ordering::Release);
            }
            for (v, &mid) in engine.modulator_ids.iter().enumerate() {
                self.app.state.runtime.engine_modulator_node_ids[engine_id][v]
                    .store(mid as u32, Ordering::Release);
            }
        }
        Ok(())
    }

    fn connect_engine_to_track(
        &mut self,
        engine_id: usize,
        route_idx: usize,
        track_idx: usize,
        track_name: &str,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_out_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        if engine_id >= self.app.state.runtime.engine_route_lids.len() {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} has no audio-thread route table"
            ));
        }
        if route_idx >= crate::sequencer::MAX_SAMPLER_POOLS {
            return Err(format!(
                "connect_engine_to_track: route consumer {route_idx} exceeds the custom route limit"
            ));
        }
        let Some(existing_engine) = self.app.graph.engine_node_ids[engine_id].as_ref() else {
            return Err(format!(
                "connect_engine_to_track: missing engine runtime for engine {}",
                engine_id
            ));
        };
        if lisp_host::get_dgen_engine_enabled_voices(engine_id) == 0 {
            lisp_host::reset_dgen_engine_enabled_voices(engine_id);
        }
        let (Some(existing_routes), Some(existing_ext_routes)) = (
            existing_engine.route_gain_ids.get(route_idx),
            existing_engine.ext_route_gain_ids.get(route_idx),
        ) else {
            return Err(format!(
                "connect_engine_to_track: track {track_idx} is outside engine {engine_id}'s route table"
            ));
        };
        if existing_routes.len() == MAX_VOICES && existing_ext_routes.len() == MAX_VOICES {
            return Ok(());
        }
        if !existing_routes.is_empty() || !existing_ext_routes.is_empty() {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} track {track_idx} has incomplete route metadata"
            ));
        }
        let synth_ids = existing_engine.synth_ids.clone();
        let has_sibling_route =
            existing_engine
                .route_gain_ids
                .iter()
                .enumerate()
                .any(|(idx, routes)| {
                    idx != route_idx
                        && !routes.is_empty()
                        && custom_route_parent_track(idx) == Some(track_idx)
                });
        let audio_output_channels = existing_engine.audio_output_channels.clone();
        let primary_mod_output_channel = existing_engine.mod_output_channels.first().copied();
        let modulator_ids = existing_engine.modulator_ids.clone();
        if synth_ids.len() != MAX_VOICES {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} has {} synth voices, expected {MAX_VOICES}",
                synth_ids.len()
            ));
        }
        if !modulator_ids.is_empty() && modulator_ids.len() != MAX_VOICES {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} has {} modulator voices, expected 0 or {MAX_VOICES}",
                modulator_ids.len()
            ));
        }

        let (route_node_capacity, route_connection_capacity) =
            engine_route_build_capacities(existing_engine);
        let mut transaction = GraphNodeBuildTransaction::new(
            self.app.graph.lg.0,
            route_node_capacity,
            route_connection_capacity,
        )?;

        let mut route_ids = Vec::with_capacity(MAX_VOICES);
        let mut ext_route_ids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let route_l_id = transaction.own(add_engine_route_gain_node_checked(
                self.app.graph.lg.0,
                0.0,
                &format!("{}_eng{}_route_{}_l", track_name, engine_id, v),
                &format!(
                    "connect_engine_to_track left route engine {} track {} voice {}",
                    engine_id, track_idx, v
                ),
            )?)?;
            if let Some(src_channel) = stereo_route_source_channel(&audio_output_channels, 0) {
                transaction.connect(
                    synth_ids[v],
                    src_channel as i32,
                    route_l_id,
                    0,
                    &format!(
                        "connect_engine_to_track left engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
            }
            transaction.connect(
                route_l_id,
                0,
                voice_sum_id,
                0,
                &format!(
                    "connect_engine_to_track left engine {} track {} voice {}",
                    engine_id, track_idx, v
                ),
            )?;

            let mut route_pair = [route_l_id, 0];

            if let Some(src_channel) = stereo_route_source_channel(&audio_output_channels, 1) {
                let route_r_id = transaction.own(add_engine_route_gain_node_checked(
                    self.app.graph.lg.0,
                    0.0,
                    &format!("{}_eng{}_route_{}_r", track_name, engine_id, v),
                    &format!(
                        "connect_engine_to_track right route engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?)?;
                transaction.connect(
                    synth_ids[v],
                    src_channel as i32,
                    route_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                transaction.connect(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                route_pair[1] = route_r_id;
            } else {
                let route_r_id = transaction.own(add_engine_route_gain_node_checked(
                    self.app.graph.lg.0,
                    0.0,
                    &format!("{}_eng{}_route_{}_r", track_name, engine_id, v),
                    &format!(
                        "connect_engine_to_track mirrored right route engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?)?;
                transaction.connect(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track mirrored-right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                route_pair[1] = route_r_id;
            }

            route_ids.push(route_pair);

            if !has_sibling_route {
                if let Some(src_channel) = primary_mod_output_channel {
                    transaction.connect(
                        synth_ids[v],
                        src_channel as i32,
                        track_mod_out_id,
                        0,
                        &format!(
                            "connect_engine_to_track mod output engine {} track {} voice {}",
                            engine_id, track_idx, v
                        ),
                    )?;
                }
            }

            let mut voice_ext_route_ids = [0; EXT_MOD_INPUT_COUNT];
            for input in 0..EXT_MOD_INPUT_COUNT {
                if !modulator_ids.is_empty() {
                    let ext_route_id = transaction.own(add_engine_route_gain_node_checked(
                        self.app.graph.lg.0,
                        0.0,
                        &format!(
                            "{}_eng{}_ext{}_route_{}",
                            track_name,
                            engine_id,
                            input + 1,
                            v
                        ),
                        &format!(
                            "connect_engine_to_track ext{} route engine {} track {} voice {}",
                            input + 1,
                            engine_id,
                            track_idx,
                            v
                        ),
                    )?)?;
                    transaction.connect(
                        track_mod_in_clip_ids[input],
                        0,
                        ext_route_id,
                        0,
                        &format!(
                            "connect_engine_to_track ext{} input engine {} track {} voice {}",
                            input + 1,
                            engine_id,
                            track_idx,
                            v
                        ),
                    )?;
                    transaction.connect(
                        ext_route_id,
                        0,
                        modulator_ids[v],
                        4 + input as i32,
                        &format!(
                            "connect_engine_to_track ext{} modulator engine {} track {} voice {}",
                            input + 1,
                            engine_id,
                            track_idx,
                            v
                        ),
                    )?;
                    voice_ext_route_ids[input] = ext_route_id;
                }
            }
            ext_route_ids.push(voice_ext_route_ids);
        }

        if self.app.graph.engine_node_ids[engine_id].is_none() {
            return Err(format!(
                "connect_engine_to_track: engine runtime disappeared for engine {}",
                engine_id
            ));
        }
        transaction.commit();
        let engine = self.app.graph.engine_node_ids[engine_id]
            .as_mut()
            .expect("engine runtime was validated immediately before transaction commit");
        engine.route_gain_ids[route_idx] = route_ids;
        engine.ext_route_gain_ids[route_idx] = ext_route_ids;
        for voice in 0..MAX_VOICES {
            let [route_l_id, route_r_id] = engine.route_gain_ids[route_idx][voice];
            if route_idx < MAX_TRACKS {
                self.app.state.runtime.engine_route_lids[engine_id][voice][route_idx]
                    .store(route_l_id as u64, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][route_idx]
                    .store(route_r_id as u64, Ordering::Release);
            } else {
                self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                    .store(engine_id as u32, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                    .store(route_l_id as u64, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                    .store(route_r_id as u64, Ordering::Release);
            }
            for input in 0..EXT_MOD_INPUT_COUNT {
                let ext_route_id = engine.ext_route_gain_ids[route_idx][voice][input];
                if route_idx < MAX_TRACKS {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][route_idx]
                        [input]
                        .store(ext_route_id as u64, Ordering::Release);
                } else {
                    self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                        .store(ext_route_id as u64, Ordering::Release);
                }
            }
        }
        Ok(())
    }

    fn silence_engine_routes(&self, engine_id: usize, engine: &EngineNodeIds) {
        for voice_idx in 0..MAX_VOICES {
            let lid = self.app.state.runtime.engine_voice_lids[engine_id][voice_idx]
                .load(Ordering::Acquire);
            if lid != 0 {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::effects::gatepitch::PARAM_GATE,
                            logical_id: lid,
                            fvalue: 0.0,
                        },
                    );
                }
            }
        }

        for route_pair in engine
            .route_gain_ids
            .iter()
            .flat_map(|routes| routes.iter())
        {
            for &route_id in route_pair {
                if route_id <= 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: 0,
                            logical_id: route_id as u64,
                            fvalue: 0.0,
                        },
                    );
                }
            }
        }
    }

    fn rebuild_custom_engine_runtime(
        &mut self,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        let host_routes = custom_host_input_routes(
            manifest,
            &format!("rebuild_custom_engine_runtime engine {engine_id}"),
        )?;
        let Some(mut engine) = self.app.graph.engine_node_ids[engine_id].take() else {
            return Err("Missing engine runtime".to_string());
        };
        self.silence_engine_routes(engine_id, &engine);
        lisp_host::reset_dgen_engine_enabled_voices(engine_id);

        let audio_output_channels = manifest_audio_output_channels(manifest);
        let mod_output_channels = manifest_mod_output_channels(manifest);
        let primary_mod_output_channel = mod_output_channels.first().copied();

        let mut new_synth_ids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let old_synth = engine.synth_ids[v];
            let gp_id = engine.gatepitch_ids[v];
            let mod_id = engine.modulator_ids[v];

            unsafe {
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 0, old_synth, 0);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 1, old_synth, 1);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 2, old_synth, 2);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 3, old_synth, 3);
                for mod_out in 0..crate::voice_modulator::NUM_OUTPUTS {
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        mod_id,
                        mod_out as i32,
                        old_synth,
                        4 + mod_out as i32,
                    );
                }
                for route_pair in engine
                    .route_gain_ids
                    .iter()
                    .filter_map(|routes| routes.get(v))
                {
                    for (route_idx, &route_id) in route_pair.iter().enumerate() {
                        if route_id <= 0 {
                            continue;
                        }
                        if let Some(src_channel) =
                            stereo_route_source_channel(&engine.audio_output_channels, route_idx)
                        {
                            crate::audiograph::graph_disconnect(
                                self.app.graph.lg.0,
                                old_synth,
                                src_channel as i32,
                                route_id,
                                0,
                            );
                        }
                    }
                }
                for (input, ext_route_id) in engine
                    .ext_route_gain_ids
                    .iter()
                    .filter_map(|routes| routes.get(v))
                    .flat_map(|route_ids| route_ids.iter().enumerate())
                {
                    if *ext_route_id > 0 {
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            *ext_route_id,
                            0,
                            mod_id,
                            4 + input as i32,
                        );
                    }
                }
                crate::audiograph::delete_node(self.app.graph.lg.0, old_synth);
            }

            let slot_id = engine_id * MAX_VOICES + v;
            lisp_host::set_dgen_instrument_fn(slot_id, lib.process_fn);
            lisp_host::set_dgen_instrument_output_count(slot_id, manifest.n_outputs.max(1));
            let init_msg = lisp_host::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_host::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();
            let synth_name = CString::new(format!("engine_{}_synth_{}", engine_id, v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_host::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    manifest.n_outputs.max(1) as i32,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };
            if synth_id < 0 {
                return Err(format!(
                    "rebuild_custom_engine_runtime: failed to add synth node for engine {} voice {} (manifest.n_inputs={})",
                    engine_id, v, manifest.n_inputs
                ));
            }
            self.connect_custom_host_inputs(
                gp_id,
                mod_id,
                synth_id,
                &host_routes,
                &format!("rebuild_custom_engine_runtime engine {engine_id} voice {v}"),
            )?;
            for route_pair in engine
                .route_gain_ids
                .iter()
                .filter_map(|routes| routes.get(v))
            {
                for (route_idx, &route_id) in route_pair.iter().enumerate() {
                    if route_id <= 0 {
                        continue;
                    }
                    if let Some(src_channel) =
                        stereo_route_source_channel(&audio_output_channels, route_idx)
                    {
                        self.graph_connect_checked(
                            synth_id,
                            src_channel as i32,
                            route_id,
                            0,
                            &format!(
                                "rebuild_custom_engine_runtime engine {} voice {} route {}:{}",
                                engine_id, v, route_id, src_channel
                            ),
                        )?;
                    }
                }
            }
            if let Some(src_channel) = primary_mod_output_channel {
                for bound_track in 0..self.app.graph.track_engine_ids.len() {
                    if self.app.graph.track_engine_ids.get(bound_track) == Some(&Some(engine_id)) {
                        let Some(track_nodes) = self.app.graph.track_node_ids.get(bound_track)
                        else {
                            continue;
                        };
                        self.graph_connect_checked(
                            synth_id,
                            src_channel as i32,
                            track_nodes.mod_out_id,
                            0,
                            &format!(
                                "rebuild_custom_engine_runtime engine {} voice {} track {} mod output {}",
                                engine_id, v, bound_track, src_channel
                            ),
                        )?;
                    }
                }
            }
            for (input, ext_route_id) in engine
                .ext_route_gain_ids
                .iter()
                .filter_map(|routes| routes.get(v))
                .flat_map(|route_ids| route_ids.iter().enumerate())
            {
                if *ext_route_id <= 0 {
                    continue;
                }
                self.graph_connect_checked(
                    *ext_route_id,
                    0,
                    mod_id,
                    4 + input as i32,
                    &format!(
                        "rebuild_custom_engine_runtime engine {} voice {} ext{} route {}",
                        engine_id,
                        v,
                        input + 1,
                        ext_route_id
                    ),
                )?;
            }

            new_synth_ids.push(synth_id);
            self.app.state.runtime.engine_synth_node_ids[engine_id][v]
                .store(synth_id as u32, Ordering::Release);
        }

        engine.synth_ids = new_synth_ids;
        engine.synth_inputs = manifest.n_inputs;
        engine.synth_outputs = audio_output_channels.len();
        engine.audio_output_channels = audio_output_channels;
        engine.mod_output_channels = mod_output_channels;
        for (v, &mid) in engine.modulator_ids.iter().enumerate() {
            self.app.state.runtime.engine_modulator_node_ids[engine_id][v]
                .store(mid as u32, Ordering::Release);
        }
        for bound_track in 0..self.app.graph.track_engine_ids.len() {
            if self.app.graph.track_engine_ids[bound_track] == Some(engine_id) {
                self.app.graph.track_synth_node_ids[bound_track] = engine.synth_ids.clone();
                self.app.graph.track_gatepitch_node_ids[bound_track] = engine.gatepitch_ids.clone();
            }
        }
        self.silence_engine_routes(engine_id, &engine);
        self.app.graph.engine_node_ids[engine_id] = Some(engine);
        Ok(())
    }

    fn finish_track_registration(
        &mut self,
        registration: TrackRegistration<'_>,
    ) -> Result<(), String> {
        self.app
            .track_registry
            .allocate()
            .map_err(|error| format!("Failed to allocate stable track id: {error:?}"))?;
        let TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids,
            instrument,
        } = registration;
        let instrument_type = match &instrument {
            InstrumentRegistration::Sampler { .. } => InstrumentType::Sampler,
            InstrumentRegistration::Custom { .. } => InstrumentType::Custom,
            InstrumentRegistration::Modulator => InstrumentType::Modulator,
        };
        let run_mode = match &instrument {
            InstrumentRegistration::Custom { run_mode, .. } => *run_mode,
            InstrumentRegistration::Sampler { .. } | InstrumentRegistration::Modulator => {
                CustomInstrumentRunMode::Instrument
            }
        };

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.runtime.voice_lids[idx][v].store(lid, Ordering::Release);
        }
        self.app.state.runtime.voice_counts[idx].store(voice_lids.len() as u32, Ordering::Release);
        self.app.state.runtime.sampler_lids[idx]
            .store(voice_lids.first().copied().unwrap_or(0), Ordering::Release);
        self.app.state.runtime.modulator_lids[idx].store(
            if instrument_type == InstrumentType::Modulator {
                shell.mod_env_id as u64
            } else {
                0
            },
            Ordering::Release,
        );
        self.app.state.runtime.pan_lids[idx].store(shell.pan_id as u64, Ordering::Release);
        self.app.state.runtime.delay_lids[idx].store(shell.delay_id as u64, Ordering::Release);
        self.app.state.runtime.send_lids[idx].store(shell.send_id as u64, Ordering::Release);
        self.app.state.runtime.instrument_type_flags[idx]
            .store(instrument_type.runtime_flag(), Ordering::Release);
        self.app.state.pattern.instrument_run_modes[idx]
            .store(run_mode.runtime_flag(), Ordering::Release);
        self.app.state.runtime.instrument_run_mode_flags[idx]
            .store(run_mode.runtime_flag(), Ordering::Release);

        self.app.tracks.push(track_name.clone());
        self.app.push_next_track_color();
        self.app.push_default_track_collapsed();
        self.app.rack_selected_slots.push(0);
        self.app.rack_pad_bank_starts.push(DRUM_RACK_FIRST_PAD_NOTE);
        self.app
            .graph
            .effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.app.graph.record_armed.push(false);
        self.app.graph.track_voice_lids.push(voice_lids);
        self.app.graph.track_instrument_types.push(instrument_type);
        self.app.graph.track_instrument_run_modes.push(run_mode);

        match instrument {
            InstrumentRegistration::Sampler {
                buffer_id,
                sample_rate,
                sampler_ids,
                gatepitch_ids,
                modulator_ids,
            } => {
                let instrument_node_id = first_graph_node_identity(&sampler_ids);
                let instrument_modulator_node_id = first_graph_node_identity(&modulator_ids);
                for (v, &sampler_id) in sampler_ids.iter().enumerate() {
                    self.app.state.runtime.synth_node_ids[idx][v]
                        .store(sampler_id as u32, Ordering::Release);
                }
                for (v, &gatepitch_id) in gatepitch_ids.iter().enumerate() {
                    self.app.state.runtime.sampler_gatepitch_node_ids[idx][v]
                        .store(gatepitch_id as u32, Ordering::Release);
                }
                for (v, &modulator_id) in modulator_ids.iter().enumerate() {
                    self.app.state.runtime.sampler_modulator_node_ids[idx][v]
                        .store(modulator_id as u32, Ordering::Release);
                }
                self.app.state.runtime.track_engine_ids[idx].store(u32::MAX, Ordering::Release);
                if let Some(sound) = self
                    .app
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(idx)
                {
                    sound.engine_id = None;
                }
                self.app.graph.track_buffer_ids.push(buffer_id);
                self.app.graph.track_sample_rates.push(sample_rate);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids,
                    sampler_gatepitch_ids: gatepitch_ids.clone(),
                    sampler_modulator_ids: modulator_ids.clone(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
                    mod_out_id: shell.mod_out_id,
                    mod_in_clip_ids: shell.mod_in_clip_ids,
                    mod_env_id: shell.mod_env_id,
                    bus_send_ids: Vec::new(),
                    rack_slots: Vec::new(),
                    rack_signature: None,
                });
                self.app.graph.track_synth_node_ids.push(Vec::new());
                self.app.graph.track_gatepitch_node_ids.push(Vec::new());
                self.app.graph.track_engine_ids.push(None);
                let sampler_desc = EffectDescriptor::builtin_sampler();
                self.app.state.pattern.instrument_slots[idx].apply_descriptor_with_modulator(
                    &sampler_desc,
                    instrument_node_id,
                    instrument_modulator_node_id,
                );
                self.app.graph.instrument_descriptors.push(sampler_desc);
            }
            InstrumentRegistration::Custom {
                engine_id,
                manifest,
                run_mode: _,
            } => {
                self.app.state.runtime.track_engine_ids[idx]
                    .store(engine_id as u32, Ordering::Release);
                if let Some(sound) = self
                    .app
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(idx)
                {
                    sound.engine_id = Some(engine_id);
                }
                self.app.graph.track_buffer_ids.push(-1);
                self.app
                    .graph
                    .track_sample_rates
                    .push(self.app.graph.sample_rate);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids: Vec::new(),
                    sampler_gatepitch_ids: Vec::new(),
                    sampler_modulator_ids: Vec::new(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
                    mod_out_id: shell.mod_out_id,
                    mod_in_clip_ids: shell.mod_in_clip_ids,
                    mod_env_id: shell.mod_env_id,
                    bus_send_ids: Vec::new(),
                    rack_slots: Vec::new(),
                    rack_signature: None,
                });
                let engine = self.app.graph.engine_node_ids[engine_id]
                    .as_ref()
                    .expect("engine runtime initialized");
                self.app
                    .graph
                    .track_synth_node_ids
                    .push(engine.synth_ids.clone());
                self.app
                    .graph
                    .track_gatepitch_node_ids
                    .push(engine.gatepitch_ids.clone());
                self.app.graph.track_engine_ids.push(Some(engine_id));
                self.initialize_instrument_slot(idx, &track_name, manifest);
            }
            InstrumentRegistration::Modulator => {
                unsafe {
                    crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, shell.mod_env_id);
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        shell.mod_env_id,
                        0,
                        shell.mod_out_id,
                        0,
                    );
                }
                self.app.state.runtime.track_engine_ids[idx].store(u32::MAX, Ordering::Release);
                if let Some(sound) = self
                    .app
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(idx)
                {
                    sound.engine_id = None;
                }
                self.app.graph.track_buffer_ids.push(-1);
                self.app
                    .graph
                    .track_sample_rates
                    .push(self.app.graph.sample_rate);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids: Vec::new(),
                    sampler_gatepitch_ids: Vec::new(),
                    sampler_modulator_ids: Vec::new(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
                    mod_out_id: shell.mod_out_id,
                    mod_in_clip_ids: shell.mod_in_clip_ids,
                    mod_env_id: shell.mod_env_id,
                    bus_send_ids: Vec::new(),
                    rack_slots: Vec::new(),
                    rack_signature: None,
                });
                self.app.graph.track_synth_node_ids.push(Vec::new());
                self.app.graph.track_gatepitch_node_ids.push(Vec::new());
                self.app.graph.track_engine_ids.push(None);
                let desc = crate::track_modulator::descriptor();
                self.app.state.pattern.instrument_slots[idx]
                    .apply_descriptor(&desc, shell.mod_env_id as u32);
                self.app.graph.instrument_descriptors.push(desc);
            }
        }

        let instrument_snapshot = if instrument_type == InstrumentType::Custom
            || instrument_type == InstrumentType::Modulator
        {
            let desc = self.app.graph.instrument_descriptors[idx].clone();
            let node_id = self.app.state.pattern.instrument_slots[idx]
                .node_id
                .load(Ordering::Relaxed);
            Some((desc, node_id, instrument_type))
        } else {
            None
        };
        self.app.state.extend_all_pattern_snapshots_to_track(
            idx + 1,
            &self.app.graph.effect_descriptors,
            idx,
            run_mode,
            instrument_snapshot
                .as_ref()
                .map(|(desc, node_id, instrument_type)| (desc, *node_id, *instrument_type)),
        );
        self.app.refresh_effect_sidechain_labels();

        self.app
            .state
            .transport
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);
        self.app.ui.cursor_track = idx;
        self.app.ui.cursor_step = 0;
        self.app.ui.focused_region = super::Region::Cirklon;
        self.app.ui.sidebar_tab = super::SidebarTab::Tools;
        self.app.ui.sidebar_mode = match instrument_type {
            InstrumentType::Custom => super::SidebarMode::Presets,
            InstrumentType::Sampler | InstrumentType::Modulator | InstrumentType::Rack => {
                super::SidebarMode::Audition
            }
        };
        self.app.ui.sidebar_search_focused = false;
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn debug_assert_track_vectors_aligned(&self) {
        debug_assert_eq!(self.app.track_registry.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.graph.track_node_ids.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.graph.track_buffer_ids.len(), self.app.tracks.len());
        debug_assert_eq!(
            self.app.graph.track_sample_rates.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(self.app.graph.track_voice_lids.len(), self.app.tracks.len());
        debug_assert_eq!(
            self.app.graph.track_instrument_types.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.track_instrument_run_modes.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(self.app.graph.track_engine_ids.len(), self.app.tracks.len());
        debug_assert_eq!(
            self.app.graph.track_synth_node_ids.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.track_gatepitch_node_ids.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.effect_descriptors.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.instrument_descriptors.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(self.app.graph.record_armed.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.sampler_paths.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.rack_selected_slots.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.rack_pad_bank_starts.len(), self.app.tracks.len());
    }

    fn initialize_instrument_slot(&mut self, track: usize, name: &str, manifest: &DGenManifest) {
        self.apply_instrument_slot_descriptor(track, name, manifest, false);
    }

    fn sync_instrument_slot(&mut self, track: usize, name: &str, manifest: &DGenManifest) {
        self.apply_instrument_slot_descriptor(track, name, manifest, true);
    }

    fn apply_instrument_slot_descriptor(
        &mut self,
        track: usize,
        name: &str,
        manifest: &DGenManifest,
        preserve_runtime_values: bool,
    ) {
        let inst_desc = lisp_host::instrument_descriptor_from_manifest(name, manifest);
        let inst_slot = &self.app.state.pattern.instrument_slots[track];
        let (node_id, modulator_node_id) = self.instrument_slot_identity(track);
        if preserve_runtime_values {
            if let Some(old_desc) = self.app.graph.instrument_descriptors.get(track) {
                inst_slot.sync_descriptor_by_param_name_with_modulator(
                    old_desc,
                    &inst_desc,
                    node_id,
                    modulator_node_id,
                );
            } else {
                inst_slot.sync_descriptor_with_modulator(&inst_desc, node_id, modulator_node_id);
            }
        } else {
            inst_slot.apply_descriptor_with_modulator(&inst_desc, node_id, modulator_node_id);
        }

        if track < self.app.graph.instrument_descriptors.len() {
            self.app.graph.instrument_descriptors[track] = inst_desc;
        } else {
            self.app.graph.instrument_descriptors.push(inst_desc);
        }
    }

    fn instrument_slot_identity(&self, track: usize) -> (u32, u32) {
        let Some(Some(engine_id)) = self.app.graph.track_engine_ids.get(track).copied() else {
            let slot = &self.app.state.pattern.instrument_slots[track];
            return (
                slot.node_id.load(Ordering::Relaxed),
                slot.modulator_node_id.load(Ordering::Relaxed),
            );
        };
        let Some(Some(engine)) = self.app.graph.engine_node_ids.get(engine_id) else {
            return (0, 0);
        };
        (
            first_graph_node_identity(&engine.synth_ids),
            first_graph_node_identity(&engine.modulator_ids),
        )
    }
}

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
mod tests {
    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::macro_engine::{MacroCurve, MacroKind, MacroMapping, MacroParamKey};
    use crate::process::ParamTarget;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{default_empty_effect_chain, SequencerState};
    use crate::tui::AudioBuses;
    use std::path::PathBuf;
    use std::sync::{mpsc, Arc, Mutex};

    fn topology_test_slot(
        instrument_type: InstrumentType,
        engine_id: Option<usize>,
    ) -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            pad_note: None,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: 4,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_default(
                &EffectDescriptor::builtin_sampler(),
                1,
            ),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState {
                engine_id,
                loaded_preset: None,
                dirty: false,
            },
            sample_id: (instrument_type == InstrumentType::Sampler)
                .then(|| (1, "kick".to_string(), 44_100)),
        }
    }

    fn topology_test_rack() -> RackTrackSnapshot {
        RackTrackSnapshot::new(
            RackRouting::Broadcast,
            vec![
                topology_test_slot(InstrumentType::Sampler, None),
                topology_test_slot(InstrumentType::Sampler, None),
            ],
            crate::sequencer::default_rack_macros(),
        )
    }

    #[test]
    fn signature_equal_for_identical_snapshots() {
        let first = topology_test_rack();
        let second = first.clone();

        assert_eq!(
            rack_topology_signature(&first),
            rack_topology_signature(&second)
        );
    }

    #[test]
    fn signature_ignores_parameter_fields() {
        let first = topology_test_rack();
        let mut second = first.clone();
        let slot = &mut second.slots[0];
        slot.gain = 0.25;
        slot.pan = -0.5;
        slot.mute = true;
        slot.solo = true;
        slot.max_polyphony = 1;
        slot.sample_id = Some((99, "other-kick".to_string(), 48_000));
        slot.pad_note = Some(36);
        slot.choke_group = Some(2);
        assert!(slot
            .param_plocks
            .set(0, crate::sequencer::RackSlotParam::Gain, 0.75));
        second.macros[0].value = 0.8;
        second.routing = RackRouting::ByPitch;

        assert_eq!(
            rack_topology_signature(&first),
            rack_topology_signature(&second)
        );
    }

    #[test]
    fn signature_detects_topology_changes() {
        let base = topology_test_rack();
        let signature = rack_topology_signature(&base);
        let assert_changed = |candidate: &RackTrackSnapshot| {
            assert_ne!(signature, rack_topology_signature(candidate));
        };

        let mut slot_count = base.clone();
        slot_count.slots.pop();
        assert_changed(&slot_count);

        let mut slot_order = RackTrackSnapshot::new(
            RackRouting::Broadcast,
            vec![
                topology_test_slot(InstrumentType::Sampler, None),
                topology_test_slot(InstrumentType::Custom, Some(7)),
            ],
            crate::sequencer::default_rack_macros(),
        );
        let ordered_signature = rack_topology_signature(&slot_order);
        slot_order.slots.swap(0, 1);
        assert_ne!(ordered_signature, rack_topology_signature(&slot_order));

        let mut instrument_type = base.clone();
        instrument_type.slots[0] = topology_test_slot(InstrumentType::Custom, Some(7));
        assert_changed(&instrument_type);

        let mut engine_id = instrument_type.clone();
        engine_id.slots[0].track_sound_state.engine_id = Some(8);
        assert_ne!(
            rack_topology_signature(&instrument_type),
            rack_topology_signature(&engine_id)
        );

        let mut run_mode = instrument_type.clone();
        run_mode.slots[0].instrument_run_mode = CustomInstrumentRunMode::FreePatch;
        assert_ne!(
            rack_topology_signature(&instrument_type),
            rack_topology_signature(&run_mode)
        );

        let mut fx_node = base.clone();
        fx_node.slots[0].effect_slots[0].node_id = 42;
        assert_changed(&fx_node);

        let mut fx_length = base.clone();
        fx_length.slots[0].effect_slots.pop();
        fx_length.slots[0].effect_descriptors.pop();
        assert_changed(&fx_length);
    }

    struct TestLiveGraph {
        ptr: LiveGraphPtr,
        block_size: i32,
        channels: usize,
    }

    struct TestProjectFile(PathBuf);

    impl Drop for TestProjectFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    impl TestLiveGraph {
        fn new(label: &str) -> Self {
            const BLOCK_SIZE: i32 = 64;
            const SAMPLE_RATE: i32 = 44_100;
            const CHANNELS: usize = 2;
            crate::audiograph::initialize_engine_for_test(BLOCK_SIZE, SAMPLE_RATE);
            let label = CString::new(label).expect("test graph label should not contain NUL");
            let ptr = unsafe {
                crate::audiograph::create_live_graph(
                    32,
                    BLOCK_SIZE,
                    label.as_ptr(),
                    CHANNELS as i32,
                )
            };
            assert!(!ptr.is_null(), "test live graph should be created");
            Self {
                ptr: LiveGraphPtr(ptr),
                block_size: BLOCK_SIZE,
                channels: CHANNELS,
            }
        }

        fn add_gain(&self, gain: f32, name: &str) -> i32 {
            add_gain_node_checked(self.ptr.0, gain, name, "test graph gain")
                .expect("test gain node should be queued")
        }

        fn add_voice_modulator(&self, engine_id: usize, voice: usize) -> i32 {
            let name = CString::new(format!("test_modulator_{voice}"))
                .expect("test modulator name should not contain NUL");
            let initial_state =
                crate::voice_modulator::custom_engine_initial_state(engine_id, voice);
            let node_id = unsafe {
                crate::audiograph::add_node(
                    self.ptr.0,
                    crate::voice_modulator::voice_modulator_vtable(),
                    crate::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    name.as_ptr(),
                    crate::voice_modulator::INPUT_COUNT as i32,
                    crate::voice_modulator::NUM_OUTPUTS as i32,
                    (&initial_state as *const crate::voice_modulator::VoiceModulatorInitialState)
                        .cast(),
                    std::mem::size_of::<crate::voice_modulator::VoiceModulatorInitialState>(),
                )
            };
            assert!(node_id >= 0, "test modulator node should be queued");
            node_id
        }

        fn process_block(&self) {
            let mut output = vec![0.0_f32; self.block_size as usize * self.channels];
            unsafe {
                self.ptr
                    .process_next_block(output.as_mut_ptr(), self.block_size);
            }
        }
    }

    impl Drop for TestLiveGraph {
        fn drop(&mut self) {
            unsafe { crate::audiograph::destroy_live_graph(self.ptr.0) };
        }
    }

    struct RouteTargets {
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_out_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    }

    fn test_app(graph: &TestLiveGraph) -> App {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        App::new(
            state,
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        )
    }

    fn test_app_with_track_count(graph: &TestLiveGraph, track_count: usize) -> App {
        let state = Arc::new(SequencerState::new(
            track_count,
            (0..track_count)
                .map(|_| default_empty_effect_chain())
                .collect(),
        ));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        App::new(
            state,
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        )
    }

    fn test_instrument_manifest() -> DGenManifest {
        DGenManifest {
            dylib_path: PathBuf::new(),
            version: 1,
            process_abi: String::new(),
            total_memory_slots: 1,
            params: Vec::new(),
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: ["gate", "pitch", "velocity", "trigger"]
                .into_iter()
                .enumerate()
                .map(|(channel, name)| lisp_host::DGenInput {
                    channel,
                    name: name.to_string(),
                })
                .collect(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 4,
            n_outputs: 1,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        }
    }

    fn install_custom_track_swap_fixture(
        app: &mut App,
        graph: &TestLiveGraph,
        track_count: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) {
        {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            app.graph_controller()
                .ensure_custom_engine_runtime(0, "old", manifest, lib)
                .expect("old engine should materialize");
        }
        let (synth_ids, gatepitch_ids, node_id, modulator_node_id) = {
            let engine = app.graph.engine_node_ids[0]
                .as_ref()
                .expect("old engine should exist");
            (
                engine.synth_ids.clone(),
                engine.gatepitch_ids.clone(),
                first_graph_node_identity(&engine.synth_ids),
                first_graph_node_identity(&engine.modulator_ids),
            )
        };
        let descriptor = lisp_host::instrument_descriptor_from_manifest("old", manifest);

        for track in 0..track_count {
            let nodes = TrackNodeIds {
                sampler_ids: Vec::new(),
                sampler_gatepitch_ids: Vec::new(),
                sampler_modulator_ids: Vec::new(),
                voice_sum_id: graph.add_gain(1.0, &format!("track_{track}_voice_sum")),
                voice_sum_r_id: graph.add_gain(1.0, &format!("track_{track}_voice_sum_r")),
                pan_id: graph.add_gain(1.0, &format!("track_{track}_pan")),
                filter_id: graph.add_gain(1.0, &format!("track_{track}_filter")),
                delay_id: graph.add_gain(1.0, &format!("track_{track}_delay")),
                send_id: graph.add_gain(1.0, &format!("track_{track}_send")),
                mod_out_id: graph.add_gain(1.0, &format!("track_{track}_mod_out")),
                mod_in_clip_ids: std::array::from_fn(|input| {
                    graph.add_gain(1.0, &format!("track_{track}_mod_in_{input}"))
                }),
                mod_env_id: graph.add_gain(1.0, &format!("track_{track}_mod_env")),
                bus_send_ids: Vec::new(),
                rack_slots: Vec::new(),
                rack_signature: None,
            };
            {
                let _batch = GraphEditBatchGuard::new(graph.ptr.0);
                app.graph_controller()
                    .connect_engine_to_track(
                        0,
                        track,
                        track,
                        &format!("Track {}", track + 1),
                        nodes.voice_sum_id,
                        nodes.voice_sum_r_id,
                        nodes.mod_out_id,
                        nodes.mod_in_clip_ids,
                    )
                    .expect("old engine route should connect");
            }
            app.tracks.push(format!("Track {}", track + 1));
            app.track_registry
                .allocate()
                .expect("allocate fixture track id");
            app.graph.track_node_ids.push(nodes);
            app.graph.track_buffer_ids.push(-1);
            app.graph.track_sample_rates.push(44_100);
            app.graph.track_voice_lids.push(Vec::new());
            app.graph
                .track_instrument_types
                .push(InstrumentType::Custom);
            app.graph
                .track_instrument_run_modes
                .push(CustomInstrumentRunMode::Instrument);
            app.graph.track_engine_ids.push(Some(0));
            app.graph.track_synth_node_ids.push(synth_ids.clone());
            app.graph
                .track_gatepitch_node_ids
                .push(gatepitch_ids.clone());
            app.graph
                .effect_descriptors
                .push(EffectDescriptor::default_full_chain());
            app.graph.instrument_descriptors.push(descriptor.clone());
            app.graph.record_armed.push(false);
            app.state
                .reset_instrument_slot_all_patterns(
                    track,
                    &descriptor,
                    node_id,
                    modulator_node_id,
                    0,
                    CustomInstrumentRunMode::Instrument,
                )
                .expect("initial instrument state should reset");
        }
        app.sync_scratch_runtime_descriptors();
    }

    fn assert_test_slot_snapshot_eq(actual: &EffectSlotSnapshot, expected: &EffectSlotSnapshot) {
        assert_eq!(actual.node_id, expected.node_id);
        assert_eq!(actual.modulator_node_id, expected.modulator_node_id);
        assert_eq!(actual.num_params, expected.num_params);
        assert_eq!(actual.defaults, expected.defaults);
        assert_eq!(actual.plocks, expected.plocks);
        assert_eq!(actual.plock_param_ids, expected.plock_param_ids);
        assert_eq!(actual.key_locks, expected.key_locks);
        assert_eq!(actual.key_lock_param_ids, expected.key_lock_param_ids);
        assert_eq!(actual.param_node_indices, expected.param_node_indices);
        assert_eq!(actual.param_node_spans, expected.param_node_spans);
        assert_eq!(actual.tensor_params.len(), expected.tensor_params.len());
    }

    fn install_test_engine(app: &mut App, graph: &TestLiveGraph) -> RouteTargets {
        let synth_ids = (0..MAX_VOICES)
            .map(|voice| graph.add_gain(1.0, &format!("test_synth_{voice}")))
            .collect();
        let modulator_ids = (0..MAX_VOICES)
            .map(|voice| graph.add_voice_modulator(0, voice))
            .collect();
        app.graph.engine_node_ids = vec![Some(EngineNodeIds {
            synth_ids,
            synth_inputs: 0,
            synth_outputs: 1,
            audio_output_channels: vec![0],
            mod_output_channels: vec![0],
            gatepitch_ids: Vec::new(),
            modulator_ids,
            route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
            ext_route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
        })];
        RouteTargets {
            voice_sum_id: graph.add_gain(1.0, "test_voice_sum"),
            voice_sum_r_id: graph.add_gain(1.0, "test_voice_sum_r"),
            track_mod_out_id: graph.add_gain(1.0, "test_track_mod_out"),
            track_mod_in_clip_ids: std::array::from_fn(|input| {
                graph.add_gain(1.0, &format!("test_track_mod_in_{input}"))
            }),
        }
    }

    fn connect_test_engine(app: &mut App, targets: &RouteTargets) -> Result<(), String> {
        app.graph_controller().connect_engine_to_track(
            0,
            0,
            0,
            "Test Track",
            targets.voice_sum_id,
            targets.voice_sum_r_id,
            targets.track_mod_out_id,
            targets.track_mod_in_clip_ids,
        )
    }

    #[test]
    fn graph_edit_batch_reports_audio_thread_application() {
        let graph = TestLiveGraph::new("graph-edit-application-watermark-test");

        unsafe { crate::audiograph::begin_graph_edit_batch(graph.ptr.0) };
        let serial = unsafe { crate::audiograph::graph_edit_current_batch_serial(graph.ptr.0) };
        assert!(serial > 0, "an open batch should expose its serial");
        graph.add_gain(1.0, "watermark_probe");
        unsafe { crate::audiograph::end_graph_edit_batch(graph.ptr.0) };

        assert!(
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(graph.ptr.0) } < serial,
            "producer commit must not be mistaken for audio-thread application"
        );
        graph.process_block();
        assert!(
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(graph.ptr.0) } >= serial,
            "processing the next block should acknowledge the committed batch"
        );
    }

    #[test]
    fn track_registration_and_deletion_keep_stable_registry_aligned() {
        let graph = TestLiveGraph::new("stable-track-registry-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("add first sampler track");
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("add second sampler track");

        let first = app.track_registry.id_at(0).expect("first stable id");
        let second = app.track_registry.id_at(1).expect("second stable id");
        assert_ne!(first, second);
        assert_eq!(app.track_registry.index_of(second), Some(1));

        app.graph_controller()
            .delete_track(0)
            .expect("delete first track");
        assert_eq!(app.track_registry.ids(), &[second]);
        assert_eq!(app.track_registry.index_of(second), Some(0));
        assert_eq!(app.track_registry.len(), app.tracks.len());
        graph.process_block();
    }

    #[test]
    fn recorded_sampler_track_creation_undoes_and_redoes_with_stable_identity() {
        let graph = TestLiveGraph::new("track-creation-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("seed sampler track");
        let created = app.graph_controller()
            .add_blank_sampler_track()
            .expect("create sampler track");
        let created_id = app.track_registry.id_at(created).expect("created stable id");
        app.commit_created_track(created, "Add sampler track")
            .expect("record creation");

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.track_registry.index_of(created_id), None);

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks.len(), 2);
        assert_eq!(app.track_registry.index_of(created_id), Some(1));
        assert_eq!(
            app.graph.track_instrument_types[1],
            InstrumentType::Sampler
        );
        graph.process_block();
    }

    #[test]
    fn recorded_modulator_track_creation_round_trips() {
        let graph = TestLiveGraph::new("modulator-track-creation-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track().unwrap();
        let created = app.graph_controller().add_modulator_track().unwrap();
        let created_id = app.track_registry.id_at(created).unwrap();
        app.commit_created_track(created, "Add modulator track").unwrap();

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(created_id), Some(1));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Modulator);
        assert_ne!(app.state.runtime.modulator_lids[1].load(Ordering::Acquire), 0);
        graph.process_block();
    }

    #[test]
    fn recorded_middle_track_deletion_restores_order_identity_and_pattern_lane() {
        let graph = TestLiveGraph::new("track-deletion-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        for _ in 0..3 {
            app.graph_controller().add_blank_sampler_track()
                .expect("seed sampler track");
        }
        app.tracks[0] = "First".to_string();
        app.tracks[1] = "Deleted".to_string();
        app.tracks[2] = "Last".to_string();
        app.state.pattern.patterns[1].set_step_active(7, true);
        let effect_slot = app.add_builtin_effect_sync(1, "OTT")
            .expect("add retained track effect");
        let compressor_slot = app.add_builtin_effect_sync(0, "compressor")
            .expect("add sidechain compressor");
        let sidechain_param = app.graph.effect_descriptors[0][compressor_slot].params.iter()
            .position(|param| matches!(
                param.host_control,
                Some(crate::effects::HostControl::FxSidechain { .. })
            )).expect("compressor sidechain parameter");
        app.state.pattern.effect_chains[0][compressor_slot]
            .defaults.set(sidechain_param, 2.0);
        app.groups.push(crate::project::ProjectTrackGroup {
            id: 9,
            name: "All tracks".to_string(),
            color: [0.2, 0.3, 0.4],
            collapsed: false,
            members: vec![0, 1, 2],
            bus_id: crate::sequencer::DEFAULT_BUS_A_ID,
        });
        let ids = app.track_registry.ids().to_vec();

        app.delete_track_recorded(1).expect("delete middle track");
        assert_eq!(app.tracks, ["First", "Last"]);
        assert_eq!(app.track_registry.ids(), &[ids[0], ids[2]]);
        assert_eq!(app.groups[0].members, [0, 1]);
        assert_eq!(
            app.state.pattern.effect_chains[0][compressor_slot]
                .defaults.get(sidechain_param).to_bits(),
            1.0f32.to_bits(),
        );

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks, ["First", "Deleted", "Last"]);
        assert_eq!(app.track_registry.ids(), ids.as_slice());
        assert_eq!(app.groups[0].members, [0, 1, 2]);
        assert!(app.state.pattern.patterns[1].is_active(7));
        assert_eq!(app.graph.effect_descriptors[1][effect_slot].name, "OTT");
        assert_ne!(
            app.state.pattern.effect_chains[1][effect_slot]
                .node_id.load(Ordering::Relaxed),
            0,
        );
        assert_eq!(
            app.state.pattern.effect_chains[0][compressor_slot]
                .defaults.get(sidechain_param).to_bits(),
            2.0f32.to_bits(),
        );

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks, ["First", "Last"]);
        assert_eq!(app.track_registry.ids(), &[ids[0], ids[2]]);
        graph.process_block();
    }

    #[test]
    fn recorded_rack_track_deletion_restores_slot_effect_graph() {
        let graph = TestLiveGraph::new("rack-track-deletion-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        for _ in 0..3 {
            app.graph_controller().add_blank_sampler_track()
                .expect("seed sampler track");
        }
        app.graph_controller().group_track_to_instrument_rack(1)
            .expect("build rack track");
        app.add_builtin_rack_slot_effect_sync(1, 0, "OTT")
            .expect("add rack slot effect");
        let rack_id = app.track_registry.id_at(1).unwrap();

        app.delete_track_recorded(1).expect("delete rack track");
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(rack_id), Some(1));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Rack);
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[1]
            .clone().expect("rack state restored");
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].effect_descriptors[0].name, "OTT");
        assert_ne!(rack.slots[0].effect_slots[0].node_id, 0);
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(rack_id), None);
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[1]
                .as_ref().unwrap().slots[0].effect_descriptors[0].name,
            "OTT",
        );
        graph.process_block();
    }

    #[test]
    fn recorded_custom_track_deletion_rebuilds_engine_route_at_original_index() {
        let graph = TestLiveGraph::new("custom-track-deletion-history-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track().unwrap();
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "history-engine".to_string(),
            source: "history-engine.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.graph_controller().add_custom_track(
            "history-engine",
            engine_id,
            &manifest,
            &lib,
            CustomInstrumentRunMode::Instrument,
        ).expect("add custom track");
        app.editor.instrument_libs.push(lib);
        app.graph_controller().add_blank_sampler_track().unwrap();
        let custom_id = app.track_registry.id_at(1).unwrap();

        app.delete_track_recorded(1).expect("delete custom track");
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(custom_id), Some(1));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[1], Some(engine_id));
        let engine = app.graph.engine_node_ids[engine_id].as_ref()
            .expect("engine runtime restored");
        assert_eq!(engine.route_gain_ids[1].len(), MAX_VOICES);
        assert!(engine.route_gain_ids[1].iter().all(|route| route[0] > 0 && route[1] > 0));
        graph.process_block();
    }

    #[test]
    fn free_patch_idle_route_stays_closed_while_transport_is_stopped() {
        assert_eq!(free_patch_idle_route_value(2, 2, false), 0.0);
        assert_eq!(free_patch_idle_route_value(1, 2, false), 0.0);
    }

    #[test]
    fn free_patch_idle_route_opens_only_target_track_while_transport_is_playing() {
        assert_eq!(free_patch_idle_route_value(2, 2, true), 1.0);
        assert_eq!(free_patch_idle_route_value(1, 2, true), 0.0);
    }

    #[test]
    fn ensure_custom_engine_runtime_rolls_back_before_runtime_publication() {
        let graph = TestLiveGraph::new("engine-materialization-rollback-test");
        let mut app = test_app(&graph);
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        set_test_graph_build_failure_after(4);

        let error = {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            app.graph_controller()
                .ensure_custom_engine_runtime(0, "test", &manifest, &lib)
                .expect_err("injected engine materialization failure should be returned")
        };
        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(app.graph.engine_node_ids.len(), 1);
        assert!(app.graph.engine_node_ids[0].is_none());
        assert_eq!(
            app.state.runtime.engine_voice_counts[0].load(Ordering::Acquire),
            0
        );
        for voice in 0..MAX_VOICES {
            assert_eq!(
                app.state.runtime.engine_voice_lids[0][voice].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                app.state.runtime.engine_synth_node_ids[0][voice].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                app.state.runtime.engine_modulator_node_ids[0][voice].load(Ordering::Acquire),
                0
            );
        }

        let created_nodes = take_test_graph_build_node_ids();
        let rolled_back_nodes = take_test_graph_build_rollback_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            rolled_back_nodes,
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        let created_connections = take_test_graph_build_connections();
        assert!(!created_connections.is_empty());
        assert_eq!(
            take_test_graph_build_rollback_connections(),
            created_connections
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>()
        );

        for &node_id in &created_nodes {
            assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, node_id) });
        }
        graph.process_block();
        for node_id in created_nodes {
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            assert!(!unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    node_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            });
            assert_eq!(state_size, 0);
        }
    }

    #[test]
    fn ensure_custom_engine_runtime_publishes_only_complete_voice_pool() {
        let graph = TestLiveGraph::new("engine-materialization-commit-test");
        let mut app = test_app(&graph);
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        begin_test_graph_build_capture();

        {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            app.graph_controller()
                .ensure_custom_engine_runtime(0, "test", &manifest, &lib)
                .expect("engine materialization should succeed");
        }

        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("complete engine should be published");
        assert_eq!(engine.gatepitch_ids.len(), MAX_VOICES);
        assert_eq!(engine.modulator_ids.len(), MAX_VOICES);
        assert_eq!(engine.synth_ids.len(), MAX_VOICES);
        assert_eq!(
            app.state.runtime.engine_voice_counts[0].load(Ordering::Acquire),
            MAX_VOICES as u32
        );
        for voice in 0..MAX_VOICES {
            assert_eq!(
                app.state.runtime.engine_voice_lids[0][voice].load(Ordering::Acquire),
                engine.gatepitch_ids[voice] as u64
            );
            assert_eq!(
                app.state.runtime.engine_synth_node_ids[0][voice].load(Ordering::Acquire),
                engine.synth_ids[voice] as u32
            );
            assert_eq!(
                app.state.runtime.engine_modulator_node_ids[0][voice].load(Ordering::Acquire),
                engine.modulator_ids[voice] as u32
            );
        }
        assert_eq!(take_test_graph_build_node_ids().len(), MAX_VOICES * 3);
        assert!(take_test_graph_build_rollback_node_ids().is_empty());
        assert!(take_test_graph_build_rollback_connections().is_empty());
        graph.process_block();
    }

    #[test]
    fn swap_custom_track_rebinds_only_the_target_and_collects_unreferenced_runtime() {
        let graph = TestLiveGraph::new("custom-track-swap-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 2);
        install_custom_track_swap_fixture(&mut app, &graph, 2, &manifest, &lib);
        let track_zero_sum = app.graph.track_node_ids[0].voice_sum_id;
        let track_one_slot_before =
            EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[1]);
        let macro_id = app
            .macro_engine
            .create_macro("swap cleanup", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                macro_id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::InstrumentParam {
                        param: "old tone".to_string(),
                        param_id: None,
                    },
                    Some(0),
                    0.0,
                    1.0,
                    MacroCurve::Linear,
                )
                .expect("instrument mapping"),
            )
            .expect("instrument mapping should attach");
        app.macro_engine
            .add_mapping(
                macro_id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::EffectParam {
                        slot: 0,
                        effect: "filter".to_string(),
                        param: "enabled".to_string(),
                        param_id: None,
                    },
                    Some(0),
                    0.2,
                    0.8,
                    MacroCurve::Linear,
                )
                .expect("effect mapping"),
            )
            .expect("effect mapping should attach");
        app.macro_engine.set_value(macro_id, 0.5);
        app.state
            .publish_macro_overrides(app.macro_engine.override_snapshot());

        let summary = app
            .graph_controller()
            .swap_custom_track_instrument(
                0,
                "new",
                1,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("first track should swap to the new engine");
        assert_eq!(summary.patterns_reset, 1);
        assert_eq!(app.graph.track_engine_ids, vec![Some(1), Some(0)]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, track_zero_sum);
        assert_eq!(
            app.tracks[0], "new",
            "track name should follow the replacement instrument"
        );
        let macro_mappings = &app
            .macro_engine
            .macro_definition(macro_id)
            .expect("macro should survive the swap")
            .mappings;
        assert_eq!(macro_mappings.len(), 1);
        assert!(matches!(
            &macro_mappings[0].target,
            ParamTarget::EffectParam { .. }
        ));
        assert_eq!(
            app.macro_engine
                .override_value(&MacroParamKey::Instrument { track: 0, param: 0 }),
            None
        );
        assert!(app
            .macro_engine
            .override_value(&MacroParamKey::Effect {
                track: 0,
                slot: 0,
                param: 0,
            })
            .is_some_and(|value| (value - 0.5).abs() < 1.0e-6));
        let old_engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("shared old engine must remain for track 2");
        assert!(old_engine.route_gain_ids[0].is_empty());
        assert_eq!(old_engine.route_gain_ids[1].len(), MAX_VOICES);
        let new_engine = app.graph.engine_node_ids[1]
            .as_ref()
            .expect("new engine should be materialized");
        assert_eq!(new_engine.route_gain_ids[0].len(), MAX_VOICES);
        assert!(new_engine.route_gain_ids[1].is_empty());
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[1]),
            &track_one_slot_before,
        );
        assert_eq!(
            app.state.runtime.track_engine_ids[1].load(Ordering::Acquire),
            0
        );

        app.graph_controller()
            .swap_custom_track_instrument(
                1,
                "new",
                1,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("second track should swap to the already materialized engine");
        assert_eq!(app.graph.track_engine_ids, vec![Some(1), Some(1)]);
        assert!(
            app.graph.engine_node_ids[0].is_none(),
            "unreferenced old graph runtime should be collected"
        );
        let new_engine = app.graph.engine_node_ids[1]
            .as_ref()
            .expect("new engine should remain live");
        assert_eq!(new_engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(new_engine.route_gain_ids[1].len(), MAX_VOICES);
        graph.process_block();
    }

    #[test]
    fn sampler_track_converts_to_custom_instrument_without_replacing_its_shell() {
        let graph = TestLiveGraph::new("sampler-to-custom-conversion-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        let voice_sum_id = app.graph.track_node_ids[0].voice_sum_id;
        let voice_sum_r_id = app.graph.track_node_ids[0].voice_sum_r_id;
        let sampler_ids = app.graph.track_node_ids[0].sampler_ids.clone();

        let summary = app
            .graph_controller()
            .replace_track_with_custom_instrument(
                0,
                "new",
                0,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("sampler track should convert to a custom instrument");

        assert_eq!(summary.patterns_reset, 1);
        assert_eq!(app.tracks, vec!["new".to_string()]);
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Custom]
        );
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, voice_sum_id);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_r_id, voice_sum_r_id);
        assert!(app.graph.track_node_ids[0].sampler_ids.is_empty());
        assert!(app.graph.track_voice_lids[0].is_empty());
        assert_eq!(app.graph.track_buffer_ids[0], -1);
        assert_eq!(app.state.runtime.voice_counts[0].load(Ordering::Acquire), 0);
        assert!(sampler_ids
            .iter()
            .all(|node_id| { !app.graph.track_node_ids[0].sampler_ids.contains(node_id) }));
        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("new custom engine should remain live");
        assert_eq!(engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(
            app.state.export_pattern_repository()[0].sample_ids[0],
            (-1, String::new(), 44_100)
        );
        graph.process_block();
    }

    #[test]
    fn grouping_sampler_track_moves_insert_chain_into_rack_slot_without_replacing_shell() {
        let graph = TestLiveGraph::new("sampler-group-to-rack-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        let voice_sum_id = app.graph.track_node_ids[0].voice_sum_id;
        let voice_sum_r_id = app.graph.track_node_ids[0].voice_sum_r_id;
        let pan_id = app.graph.track_node_ids[0].pan_id;
        let delay_id = app.graph.track_node_ids[0].delay_id;
        let effect_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("OTT should be inserted on the flat track");
        let effect_node = app.state.pattern.effect_chains[0][effect_slot]
            .node_id
            .load(Ordering::Relaxed);
        assert_ne!(effect_node, 0);

        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("flat sampler should group to a one-slot rack");

        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, voice_sum_id);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_r_id, voice_sum_r_id);
        assert_eq!(app.graph.track_node_ids[0].pan_id, pan_id);
        assert_eq!(app.graph.track_node_ids[0].delay_id, delay_id);
        assert_eq!(app.graph.track_node_ids[0].rack_slots.len(), 1);
        assert_eq!(
            app.state.pattern.effect_chains[0][effect_slot]
                .node_id
                .load(Ordering::Relaxed),
            0,
            "track-level insert chain should be empty after grouping"
        );
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should be published");
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert_eq!(rack.slots[0].effect_descriptors[effect_slot].name, "OTT");
        graph.process_block();
    }

    #[test]
    fn rack_rebuild_defers_old_sampler_nodes_until_forced_reap() {
        let graph = TestLiveGraph::new("rack-deferred-sampler-teardown-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("flat sampler should group to a rack");

        let old_sampler_id = app.graph.track_node_ids[0].rack_slots[0].sampler_ids[0];
        assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, old_sampler_id) });
        graph.process_block();
        let mut sampler_state = vec![0.0_f32; crate::sampler::SAMPLER_STATE_SIZE];
        let mut state_size = 0;
        assert!(unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_sampler_id,
                sampler_state.as_mut_ptr().cast(),
                sampler_state.len() * std::mem::size_of::<f32>(),
                &mut state_size,
            )
        });

        let mut rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should exist");
        rack.slots.push(rack.slots[0].clone());
        app.graph_controller()
            .rebuild_rack_slot_graph(0, &mut rack)
            .expect("rack topology should rebuild");
        assert_eq!(app.graph.deferred_rack_teardowns.len(), 1);
        assert_ne!(
            app.graph.track_node_ids[0].rack_slots[0].sampler_ids[0],
            old_sampler_id
        );

        graph.process_block();
        state_size = 0;
        assert!(unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_sampler_id,
                sampler_state.as_mut_ptr().cast(),
                sampler_state.len() * std::mem::size_of::<f32>(),
                &mut state_size,
            )
        });

        app.graph_controller().force_reap_all_rack_teardowns();
        assert!(app.graph.deferred_rack_teardowns.is_empty());
        graph.process_block();
        state_size = 0;
        assert!(!unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_sampler_id,
                sampler_state.as_mut_ptr().cast(),
                sampler_state.len() * std::mem::size_of::<f32>(),
                &mut state_size,
            )
        });
        assert_eq!(state_size, 0);
    }

    #[test]
    fn adding_sampler_rack_slot_refreshes_live_topology_signature() {
        let graph = TestLiveGraph::new("rack-append-signature-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("one-slot rack should load");
        app.apply_recorded_rack_slot_add(0, "Add rack sample", |app| {
            app.graph_controller().add_sampler_slot_to_rack(0, sample)
        })
            .expect("second sampler slot should append");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should remain published");
        assert_eq!(rack.slots.len(), 2);
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&rack))
        );
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            1
        );
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            2
        );
        graph.process_block();
    }

    #[test]
    fn same_engine_rack_rebuild_replaces_only_the_rack_route_generation() {
        let graph = TestLiveGraph::new("rack-deferred-engine-route-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "deferred-engine".to_string(),
            source: "deferred-engine.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.graph_controller()
            .add_custom_track(
                "deferred-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("custom track should be created");
        app.editor.instrument_libs.push(lib);
        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("custom track should group to a rack");

        let route_idx = rack_slot_pool_index(0, 0).expect("rack route identity");
        let old_routes = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("engine runtime should exist")
            .route_gain_ids[route_idx]
            .clone();
        let mut rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should exist");
        rack.slots[0].instrument_run_mode = CustomInstrumentRunMode::FreePatch;
        app.graph_controller()
            .rebuild_rack_slot_graph(0, &mut rack)
            .expect("changed rack topology should rebuild");

        assert_eq!(app.graph.deferred_rack_teardowns.len(), 1);
        let reused_routes = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("engine runtime should remain live")
            .route_gain_ids[route_idx]
            .clone();
        assert_ne!(reused_routes, old_routes);
        assert_eq!(app.graph.deferred_rack_teardowns[0].engine_routes.len(), 1);
        assert_eq!(
            lisp_host::get_dgen_engine_enabled_voices(engine_id),
            1,
            "a reused live engine must not be retired"
        );

        app.graph_controller().force_reap_all_rack_teardowns();
        graph.process_block();
        assert!(app.graph.engine_node_ids[engine_id].is_some());
        assert_eq!(
            app.graph.engine_node_ids[engine_id]
                .as_ref()
                .expect("replacement engine runtime should survive")
                .route_gain_ids[route_idx],
            reused_routes
        );
    }

    #[test]
    fn flat_track_and_rack_slot_share_one_custom_engine_with_distinct_routes() {
        let graph = TestLiveGraph::new("shared-flat-and-rack-engine-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "shared-engine".to_string(),
            source: "shared-engine.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.editor
            .instrument_libs
            .push(lisp_host::test_loaded_dgen_lib());
        app.graph_controller()
            .add_custom_track(
                "shared-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("flat custom track should be created");
        let rack_track = app
            .graph_controller()
            .add_empty_layer_rack_track()
            .expect("rack track should be created");
        app.graph_controller()
            .add_custom_slot_to_rack(
                rack_track,
                "shared-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("rack slot should consume the existing engine");
        app.apply_recorded_rack_slot_add(rack_track, "Add rack instrument", |app| {
            app.graph_controller().add_custom_slot_to_rack(
                rack_track,
                "shared-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
        })
            .expect("a second rack slot should also consume the existing engine");

        let rack_route = rack_slot_pool_index(rack_track, 0).expect("rack route identity");
        let second_rack_route =
            rack_slot_pool_index(rack_track, 1).expect("second rack route identity");
        let engine = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("shared engine runtime should exist");
        assert_eq!(engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(engine.route_gain_ids[rack_route].len(), MAX_VOICES);
        assert_eq!(engine.route_gain_ids[second_rack_route].len(), MAX_VOICES);
        assert_eq!(
            app.state.runtime.rack_engine_route_engine_ids[rack_route].load(Ordering::Acquire),
            engine_id as u32
        );
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            1
        );
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            2
        );
        assert_eq!(
            app.graph
                .engine_node_ids
                .iter()
                .filter(|engine| engine.is_some())
                .count(),
            1,
            "rack routing must not create a second DSP engine"
        );
        graph.process_block();
    }

    #[test]
    fn rack_custom_source_replacement_replays_retained_engines() {
        let graph = TestLiveGraph::new("rack-custom-source-history-test");
        let manifest = test_instrument_manifest();
        let first_lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        for (index, name) in ["first", "second"].into_iter().enumerate() {
            let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
                name: name.to_string(),
                source: format!("{name}.lisp"),
                manifest: manifest.clone(),
                lib_index: index,
                shared_runtime: true,
            });
            assert_eq!(engine_id, index);
            app.editor
                .instrument_libs
                .push(lisp_host::test_loaded_dgen_lib());
        }
        let rack_track = app
            .graph_controller()
            .add_empty_layer_rack_track()
            .unwrap();
        app.graph_controller()
            .add_custom_slot_to_rack(
                rack_track,
                "first",
                0,
                &manifest,
                &first_lib,
                CustomInstrumentRunMode::Instrument,
            )
            .unwrap();

        app.apply_recorded_rack_slot_source_replacement(
            rack_track,
            0,
            "Replace rack instrument",
            |app| {
                app.graph_controller().replace_rack_slot_with_custom(
                    rack_track,
                    0,
                    "second",
                    1,
                    &manifest,
                    CustomInstrumentRunMode::Instrument,
                )
            },
        )
        .unwrap();
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots[0]
                .track_sound_state
                .engine_id,
            Some(1)
        );
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots[0]
                .track_sound_state
                .engine_id,
            Some(0)
        );
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots[0]
                .track_sound_state
                .engine_id,
            Some(1)
        );
        graph.process_block();
    }

    #[test]
    fn rack_teardown_queue_is_bounded_and_reaps_due_generations() {
        let graph = TestLiveGraph::new("rack-deferred-queue-test");
        let mut app = test_app_with_track_count(&graph, 0);
        for track_idx in 0..=MAX_DEFERRED_RACK_TEARDOWNS {
            app.graph_controller()
                .enqueue_deferred_rack_teardown(DeferredRackTeardown {
                    slots: Vec::new(),
                    engine_routes: Vec::new(),
                    track_idx,
                    due_at: Instant::now() + RACK_TEARDOWN_TAIL,
                });
        }
        app.graph_controller().reap_excess_rack_teardowns();
        assert_eq!(
            app.graph.deferred_rack_teardowns.len(),
            MAX_DEFERRED_RACK_TEARDOWNS
        );
        assert_eq!(app.graph.deferred_rack_teardowns[0].track_idx, 1);

        for teardown in &mut app.graph.deferred_rack_teardowns {
            teardown.due_at = Instant::now();
        }
        app.graph_controller().reap_due_rack_teardowns();
        assert!(app.graph.deferred_rack_teardowns.is_empty());
    }

    #[test]
    fn grouping_custom_track_preserves_instrument_engine_state_and_insert_fx() {
        let graph = TestLiveGraph::new("custom-group-to-rack-test");
        let mut manifest = test_instrument_manifest();
        manifest.params.push(crate::lisp_host::DGenParam {
            name: "tone".to_string(),
            cell_id: 0,
            cell_span: 1,
            default: 0.25,
            min: 0.0,
            max: 1.0,
            unit: None,
            hidden: false,
            group: None,
            env: None,
            role: None,
        });
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "test-synth".to_string(),
            source: "test-synth.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.graph_controller()
            .add_custom_track(
                "test-synth",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("custom track should be created");
        let effect_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("custom track should accept OTT");
        app.state.pattern.instrument_slots[0].defaults.set(0, 0.73);
        app.state.pattern.effect_chains[0][effect_slot]
            .defaults
            .set(0, 0.63);
        let instrument_before = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        let effect_node = app.state.pattern.effect_chains[0][effect_slot]
            .node_id
            .load(Ordering::Relaxed);
        let engine_routes_before = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("custom engine runtime")
            .route_gain_ids[0]
            .clone();

        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("custom track should group without losing its instrument");

        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(app.graph.track_engine_ids[0], None);
        assert_eq!(
            app.graph.track_node_ids[0].rack_slots[0].engine_id,
            Some(engine_id)
        );
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should be published");
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].instrument_type, InstrumentType::Custom);
        assert_eq!(rack.slots[0].track_sound_state.engine_id, Some(engine_id));
        assert_eq!(
            rack.slots[0].instrument_slot.node_id,
            instrument_before.node_id
        );
        assert_eq!(rack.slots[0].instrument_slot.defaults[0], 0.73);
        assert_eq!(rack.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert_eq!(rack.slots[0].effect_descriptors[effect_slot].name, "OTT");
        let stored_patterns = app.state.export_pattern_repository();
        assert!(stored_patterns.iter().all(|pattern| {
            pattern
                .rack_tracks
                .first()
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.first())
                .is_some_and(|slot| {
                    slot.instrument_type == InstrumentType::Custom
                        && slot.track_sound_state.engine_id == Some(engine_id)
                })
        }));
        let stored_slot = &stored_patterns[0].rack_tracks[0]
            .as_ref()
            .expect("stored rack")
            .slots[0];
        assert_eq!(stored_slot.instrument_slot.defaults[0], 0.73);
        assert_eq!(stored_slot.effect_slots[effect_slot].defaults[0], 0.63);
        let rack_route = rack_slot_pool_index(0, 0).expect("rack route identity");
        assert_eq!(
            app.graph.engine_node_ids[engine_id]
                .as_ref()
                .expect("custom engine should remain live")
                .route_gain_ids[rack_route],
            engine_routes_before,
            "grouping should move the existing engine route instead of rebuilding it"
        );
        assert!(app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("custom engine should remain live")
            .route_gain_ids[0]
            .is_empty());
        assert!(app
            .rack_slot_instrument_descriptor(&rack.slots[0])
            .is_some());
        graph.process_block();
    }

    #[test]
    fn replacing_expanded_rack_instrument_preserves_slot_fx_and_defers_old_engine() {
        let graph = TestLiveGraph::new("rack-slot-instrument-replacement-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "test-synth".to_string(),
            source: "test-synth.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.editor
            .instrument_libs
            .push(lisp_host::test_loaded_dgen_lib());
        app.graph_controller()
            .add_custom_track(
                "test-synth",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("custom track should be created");
        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("custom track should group");
        let effect_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let effect_node = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .expect("rack state")
            .slots[0]
            .effect_slots[effect_slot]
            .node_id;
        let old_slot_pan_id = app.graph.track_node_ids[0].rack_slots[0].slot_pan_id;

        app.apply_recorded_rack_slot_source_replacement(
            0,
            0,
            "Replace rack sample",
            |app| app.graph_controller().replace_rack_slot_with_sampler(
                0,
                0,
                Path::new("assets/ir/lexicon-300-rich-plate.wav"),
            ),
        )
            .expect("expanded rack instrument should be replaceable");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should remain published");
        assert_eq!(rack.slots.len(), 1, "replacement must not append a layer");
        assert_eq!(rack.slots[0].instrument_type, InstrumentType::Sampler);
        assert_eq!(rack.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert_eq!(rack.slots[0].effect_descriptors[effect_slot].name, "OTT");
        assert_eq!(app.graph.track_node_ids[0].rack_slots.len(), 1);
        assert_eq!(app.graph.track_node_ids[0].rack_slots[0].engine_id, None);
        assert!(
            app.graph.engine_node_ids[engine_id].is_some(),
            "the replaced instrument runtime must survive for the release tail"
        );
        assert_eq!(
            lisp_host::get_dgen_engine_enabled_voices(engine_id),
            0,
            "an unreferenced deferred engine must stop consuming DSP"
        );
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let undone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .unwrap();
        assert_eq!(undone.slots[0].instrument_type, InstrumentType::Custom);
        assert_eq!(undone.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .unwrap();
        assert_eq!(redone.slots[0].instrument_type, InstrumentType::Sampler);
        assert_eq!(redone.slots[0].effect_slots[effect_slot].node_id, effect_node);
        graph.process_block();
        let mut old_panner_state =
            vec![0.0_f32; crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE];
        let mut old_panner_state_size = 0;
        assert!(unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_slot_pan_id,
                old_panner_state.as_mut_ptr().cast(),
                old_panner_state.len() * std::mem::size_of::<f32>(),
                &mut old_panner_state_size,
            )
        });
        assert_eq!(
            old_panner_state[crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE as usize],
            1.0,
            "the outgoing custom slot must stop carrying future shared-engine audio"
        );
        app.graph_controller().force_reap_all_rack_teardowns();
        graph.process_block();
        assert!(
            app.graph.engine_node_ids[engine_id].is_none(),
            "the replaced instrument runtime should retire when its tail is reaped"
        );
    }

    #[test]
    fn rack_preset_save_load_and_sound_promotion_preserve_slot_fx() {
        let graph = TestLiveGraph::new("rack-preset-promotion-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let preset_name = format!("rack-preset-test-{}", std::process::id());
        let preset_path = app
            .save_rack_preset(0, &preset_name, true)
            .expect("rack preset should save");
        let _preset_guard = TestProjectFile(preset_path);

        app.delete_rack_slot_effect_slot(0, 0, 0)
            .expect("live rack effect should be removable");
        app.load_rack_preset_onto_track(0, &preset_name)
            .expect("rack preset should restore");
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("restored rack state");
        assert_eq!(restored.slots[0].effect_descriptors[0].name, "OTT");
        assert_ne!(restored.slots[0].effect_slots[0].node_id, 0);

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let undone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("pre-preset rack state should be restored");
        assert_eq!(undone.slots[0].effect_slots[0].node_id, 0);
        assert_eq!(undone.slots[0].custom_effect_names[0], None);

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack preset should be restored on redo");
        assert_eq!(redone.slots[0].effect_descriptors[0].name, "OTT");
        assert_ne!(redone.slots[0].effect_slots[0].node_id, 0);

        let sound_path = app
            .promote_preset_to_sound(0, &preset_name)
            .expect("rack preset should promote to Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());
        let sound = crate::project::load_sound_preset(&sound_path)
            .expect("promoted Sound should be readable");
        assert_eq!(
            sound.rack.slots[0].custom_effects[0].as_deref(),
            Some("builtin:OTT")
        );
        graph.process_block();
    }

    #[test]
    fn deleting_rack_slot_with_fx_removes_chain_state_and_lease_host() {
        let graph = TestLiveGraph::new("delete-rack-slot-fx-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        assert!(app
            .editor
            .effect_chain_leases
            .contains_host(FxChainLocator::RackSlot { track: 0, slot: 0 }));

        app.graph_controller()
            .delete_rack_slot(0, 0)
            .expect("rack slot with FX should delete cleanly");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack container should remain");
        assert!(rack.slots.is_empty());
        assert!(app.graph.track_node_ids[0].rack_slots.is_empty());
        assert!(!app
            .editor
            .effect_chain_leases
            .contains_host(FxChainLocator::RackSlot { track: 0, slot: 0 }));
        graph.process_block();
    }

    #[test]
    fn two_slot_rack_hosts_builtin_and_compiled_fx_independently() {
        let graph = TestLiveGraph::new("two-rack-slot-fx-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav").to_path_buf();
        app.graph_controller()
            .add_sampler_rack_track(&[sample.clone(), sample])
            .expect("two-slot rack should load");
        let builtin_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("slot 0 should accept OTT");
        let compiled_slot = app
            .add_rack_slot_effect_sync(0, 1, "stereo-tremolo")
            .expect("slot 1 should accept a compiled effect");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_ne!(before.slots[0].effect_slots[builtin_slot].node_id, 0);
        assert_ne!(before.slots[1].effect_slots[compiled_slot].node_id, 0);
        let builtin_node = before.slots[0].effect_slots[builtin_slot].node_id;

        app.delete_rack_slot_effect_slot(0, 1, compiled_slot)
            .expect("compiled effect should be removable independently");
        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(
            after.slots[0].effect_slots[builtin_slot].node_id,
            builtin_node
        );
        assert_eq!(after.slots[1].effect_slots[compiled_slot].node_id, 0);
        graph.process_block();
    }

    #[test]
    fn rack_slot_effect_reorder_moves_occupied_neighbors_and_leases_together() {
        let graph = TestLiveGraph::new("rack-slot-fx-reorder-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let ott_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let filter_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "filter")
            .expect("rack slot should accept filter");
        assert_eq!((ott_slot, filter_slot), (0, 1));
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let ott_node = before.slots[0].effect_slots[0].node_id;
        let filter_node = before.slots[0].effect_slots[1].node_id;

        app.move_rack_slot_effect_slot_sync(0, 0, 0, 1)
            .expect("occupied neighboring effects should reorder");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(after.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(after.slots[0].effect_descriptors[1].name, "OTT");
        assert_eq!(after.slots[0].effect_slots[0].node_id, filter_node);
        assert_eq!(after.slots[0].effect_slots[1].node_id, ott_node);
        graph.process_block();
    }

    #[test]
    fn deleting_rack_slot_effect_compacts_state_and_lease_slots() {
        let graph = TestLiveGraph::new("rack-slot-fx-delete-compaction-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        app.add_builtin_rack_slot_effect_sync(0, 0, "filter")
            .expect("rack slot should accept filter");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let filter_node = before.slots[0].effect_slots[1].node_id;
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&before))
        );

        app.delete_rack_slot_effect_slot(0, 0, 0)
            .expect("first effect should delete");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(after.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(after.slots[0].effect_slots[0].node_id, filter_node);
        assert_eq!(after.slots[0].effect_slots[1].node_id, 0);
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&after))
        );
        let replacement_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("the first empty compacted slot should remain installable");
        assert_eq!(replacement_slot, 1);
        app.move_rack_slot_effect_slot_sync(0, 0, 0, 1)
            .expect("the compacted lease should remain movable");
        let reordered = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&reordered))
        );
        graph.process_block();
    }

    #[test]
    fn recorded_rack_slot_effect_delete_restores_identity_values_and_macro_mapping() {
        let graph = TestLiveGraph::new("rack-slot-fx-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let effect_slot = app.apply_recorded_rack_effect_chain_mutation(
            0,
            0,
            "Add rack-slot effect",
            |app| app.add_builtin_rack_slot_effect_sync(0, 0, "filter"),
        ).expect("recorded rack filter add should succeed");
        let track_id = app.track_registry.id_at(0).unwrap();
        let rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        let effect_id = app.device_registry.rack_audio_effect(rack_slot_id, effect_slot);
        app.state.update_rack_slot_in_all_pattern_snapshots(0, 0, |slot| {
            slot.effect_slots[effect_slot].defaults[0] = 0.37;
        });
        app.state.update_rack_macros_for_all_pattern_snapshots(0, |macros| {
            macros[0].mappings.push(crate::sequencer::RackMacroMapping {
                target: crate::sequencer::RackMacroTarget::SlotEffectParam {
                    slot: 0,
                    effect_slot,
                    param: "cutoff".to_string(),
                    param_index: 0,
                },
                range_min: 0.0,
                range_max: 1.0,
                curve: crate::sequencer::RackMacroCurve::Linear,
            });
        });

        app.apply_recorded_rack_effect_chain_mutation(
            0,
            0,
            "Delete rack-slot effect",
            |app| app.delete_rack_slot_effect_slot(0, 0, effect_slot),
        ).expect("recorded rack filter delete should succeed");
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should remain live");
        assert_eq!(restored.slots[0].effect_descriptors[effect_slot].name, "Filter");
        assert_eq!(restored.slots[0].effect_slots[effect_slot].defaults[0].to_bits(), 0.37_f32.to_bits());
        assert_eq!(restored.macros[0].mappings.len(), 1);
        assert_eq!(
            app.device_registry.rack_audio_effect_location(effect_id),
            Some((rack_slot_id, effect_slot))
        );
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should remain live");
        assert_eq!(redone.slots[0].effect_slots[effect_slot].node_id, 0);
        assert!(redone.macros[0].mappings.is_empty());
        graph.process_block();
    }

    #[test]
    fn inserting_rack_slot_effect_before_existing_effect_shifts_state_and_leases() {
        let graph = TestLiveGraph::new("rack-slot-fx-insert-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let ott_node = before.slots[0].effect_slots[0].node_id;

        let inserted_slot = app
            .insert_builtin_rack_slot_effect_before_slot_sync(0, 0, 0, "filter")
            .expect("filter should insert before OTT");

        assert_eq!(inserted_slot, 0);
        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(after.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(after.slots[0].effect_descriptors[1].name, "OTT");
        assert_eq!(after.slots[0].effect_slots[1].node_id, ott_node);
        app.move_rack_slot_effect_slot_sync(0, 0, 1, 0)
            .expect("shifted OTT lease should remain movable");
        graph.process_block();
    }

    #[test]
    fn rack_slot_effect_plocks_preserve_defaults_and_node_identity() {
        let graph = TestLiveGraph::new("rack-slot-fx-plock-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let effect_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "filter")
            .expect("rack slot should accept filter");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let default = before.slots[0].effect_slots[effect_slot].defaults[1];

        app.set_rack_slot_effect_plocks(0, 0, effect_slot, &[2, 3], 1, 0.75)
            .expect("selected rack effect steps should accept p-locks");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let slot = &after.slots[0].effect_slots[effect_slot];
        assert_eq!(slot.defaults[1], default);
        assert_eq!(slot.plocks[2][1], Some(0.75));
        assert_eq!(slot.plocks[3][1], Some(0.75));
        assert!(slot.plock_param_ids[2][1].is_some());
        graph.process_block();
    }

    #[test]
    fn rack_slot_effect_options_resolve_descriptor_labels() {
        let graph = TestLiveGraph::new("rack-slot-fx-option-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let effect_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "Phaser-Flanger")
            .expect("rack slot should accept Phaser-Flanger");
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let circuit_param = rack.slots[0].effect_descriptors[effect_slot]
            .params
            .iter()
            .position(|param| param.name == "phaser circuit")
            .expect("Phaser-Flanger should expose its circuit option");

        app.set_rack_slot_effect_param_option(0, 0, effect_slot, circuit_param, "stack")
            .expect("rack option labels should route through the rack host");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(
            after.slots[0].effect_slots[effect_slot].defaults[circuit_param],
            0.0
        );
        graph.process_block();
    }

    #[test]
    fn loading_sound_replaces_instrument_container_but_preserves_track_fx() {
        let graph = TestLiveGraph::new("sound-swap-preserves-track-fx-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("target sampler track should be created");
        let track_fx_slot = app
            .add_builtin_effect_sync(0, "filter")
            .expect("target track should accept a track-level effect");
        let track_fx_node = app.state.pattern.effect_chains[0][track_fx_slot]
            .node_id
            .load(Ordering::Relaxed);
        let original_buffer_id = app.graph.track_buffer_ids[0];
        let original_name = app.tracks[0].clone();

        let ott = EffectDescriptor::builtin_ott();
        let ott_snapshot = EffectSlotSnapshot::new_default(&ott, 0);
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: "Test Rack Sound".to_string(),
                tags: vec!["test".to_string()],
                author: "test".to_string(),
            },
            track: crate::project::ProjectTrack {
                id: crate::sequencer::TrackId(1),
                color: None,
                collapsed: false,
                kind: crate::project::ProjectTrackKind::Rack {
                    routing: crate::project::ProjectRackRouting::Broadcast,
                    slots: vec![crate::project::ProjectRackTrackSlot {
                        instrument_type: crate::project::ProjectInstrumentType::Sampler,
                        sample_path: Some("assets/ir/lexicon-300-rich-plate.wav".to_string()),
                        sample_name: Some("plate".to_string()),
                        instrument_name: None,
                    }],
                },
            },
            rack: crate::project::ProjectRackTrackPattern {
                macros: crate::project::default_project_rack_macros(),
                routing: crate::project::ProjectRackRouting::Broadcast,
                slots: vec![crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode: crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: crate::project::ProjectEffectSlot::default(),
                    effect_slots: vec![crate::project::ProjectEffectSlot::from(&ott_snapshot)],
                    custom_effects: vec![Some("builtin:OTT".to_string())],
                    track_sound_state: crate::project::ProjectTrackSoundState::default(),
                    sample_path: Some("assets/ir/lexicon-300-rich-plate.wav".to_string()),
                    sample_name: Some("plate".to_string()),
                }],
            },
        };
        let sound_path = std::env::temp_dir().join(format!(
            "eseq-sound-swap-{}-{}.sound",
            std::process::id(),
            track_fx_node
        ));
        std::fs::write(
            &sound_path,
            serde_json::to_string(&sound).expect("serialize test Sound"),
        )
        .expect("write test Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());

        app.load_sound_onto_track(0, &sound_path)
            .expect("Sound should load onto the target track");

        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.effect_chains[0][track_fx_slot]
                .node_id
                .load(Ordering::Relaxed),
            track_fx_node,
            "Sound swap must not replace track-level FX"
        );
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("Sound rack should be live");
        assert_ne!(rack.slots[0].effect_slots[0].node_id, 0);
        assert_eq!(rack.slots[0].effect_descriptors[0].name, "OTT");

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Sampler);
        assert_eq!(app.graph.track_buffer_ids[0], original_buffer_id);
        assert_eq!(app.tracks[0], original_name);
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        assert_eq!(
            app.state.pattern.effect_chains[0][track_fx_slot]
                .node_id.load(Ordering::Relaxed),
            track_fx_node,
            "undo must preserve the track-level effect chain",
        );

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.effect_chains[0][track_fx_slot]
                .node_id.load(Ordering::Relaxed),
            track_fx_node,
        );
        graph.process_block();
    }

    #[test]
    fn loading_sound_over_rack_undoes_as_one_container_replacement() {
        let graph = TestLiveGraph::new("sound-rack-replacement-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample_path = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample_path.to_path_buf()])
            .expect("target rack should be created");
        let filter_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "Filter")
            .expect("target rack should accept Filter");
        app.set_rack_slot_effect_param(0, 0, filter_slot, 2, 2_345.0)
            .expect("target rack Filter should accept a cutoff value");
        let track_id = app.track_registry.id_at(0).unwrap();
        let original_rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        let original_effect_id = app.device_registry
            .rack_audio_effect(original_rack_slot_id, filter_slot);

        let ott = EffectDescriptor::builtin_ott();
        let ott_snapshot = EffectSlotSnapshot::new_default(&ott, 0);
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: "Replacement Rack".to_string(),
                tags: Vec::new(),
                author: "test".to_string(),
            },
            track: crate::project::ProjectTrack {
                id: crate::sequencer::TrackId(1),
                color: None,
                collapsed: false,
                kind: crate::project::ProjectTrackKind::Rack {
                    routing: crate::project::ProjectRackRouting::Broadcast,
                    slots: (0..2).map(|slot| crate::project::ProjectRackTrackSlot {
                        instrument_type: crate::project::ProjectInstrumentType::Sampler,
                        sample_path: Some(sample_path.to_string_lossy().into_owned()),
                        sample_name: Some(format!("replacement-{}", slot + 1)),
                        instrument_name: None,
                    }).collect(),
                },
            },
            rack: crate::project::ProjectRackTrackPattern {
                macros: crate::project::default_project_rack_macros(),
                routing: crate::project::ProjectRackRouting::Broadcast,
                slots: (0..2).map(|slot| crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode: crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: crate::project::ProjectEffectSlot::default(),
                    effect_slots: vec![crate::project::ProjectEffectSlot::from(&ott_snapshot)],
                    custom_effects: vec![Some("builtin:OTT".to_string())],
                    track_sound_state: crate::project::ProjectTrackSoundState::default(),
                    sample_path: Some(sample_path.to_string_lossy().into_owned()),
                    sample_name: Some(format!("replacement-{}", slot + 1)),
                }).collect(),
            },
        };
        let sound_path = std::env::temp_dir().join(format!(
            "eseq-sound-rack-history-{}-{}.sound",
            std::process::id(),
            original_effect_id.0,
        ));
        std::fs::write(
            &sound_path,
            serde_json::to_string(&sound).expect("serialize replacement Sound"),
        ).expect("write replacement Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());

        app.load_sound_onto_track(0, &sound_path)
            .expect("replacement Sound should load");
        let replacement = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("replacement rack should be live");
        assert_eq!(replacement.slots.len(), 2);
        assert_eq!(replacement.slots[0].effect_descriptors[0].name, "OTT");
        let replacement_rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        assert_ne!(replacement_rack_slot_id, original_rack_slot_id);

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("original rack should be restored");
        assert_eq!(restored.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(restored.slots[0].effect_slots[0].defaults[2].to_bits(), 2_345.0_f32.to_bits());
        assert_eq!(
            app.device_registry.rack_slot_location(original_rack_slot_id),
            Some((track_id, 0)),
        );
        assert_eq!(
            app.device_registry.rack_audio_effect_location(original_effect_id),
            Some((original_rack_slot_id, 0)),
        );

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("replacement rack should be restored on redo");
        assert_eq!(redone.slots.len(), 2);
        assert_eq!(redone.slots[0].effect_descriptors[0].name, "OTT");
        assert_eq!(
            app.device_registry.rack_slot_location(replacement_rack_slot_id),
            Some((track_id, 0)),
        );
        graph.process_block();
    }

    #[test]
    fn loading_sound_rack_over_custom_instrument_undoes_and_redoes() {
        let graph = TestLiveGraph::new("sound-rack-over-custom-history-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "old".to_string(),
            source: "old.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        assert_eq!(engine_id, 0);
        app.editor.instrument_libs.push(lisp_host::test_loaded_dgen_lib());
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let track_id = app.track_registry.id_at(0).expect("track id");
        let original_name = app.tracks[0].clone();

        let sample_path = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: "Sampler Rack".to_string(),
                tags: Vec::new(),
                author: "test".to_string(),
            },
            track: crate::project::ProjectTrack {
                id: track_id,
                color: None,
                collapsed: false,
                kind: crate::project::ProjectTrackKind::Rack {
                    routing: crate::project::ProjectRackRouting::Broadcast,
                    slots: vec![crate::project::ProjectRackTrackSlot {
                        instrument_type: crate::project::ProjectInstrumentType::Sampler,
                        sample_path: Some(sample_path.to_string_lossy().into_owned()),
                        sample_name: Some("rack sample".to_string()),
                        instrument_name: None,
                    }],
                },
            },
            rack: crate::project::ProjectRackTrackPattern {
                macros: crate::project::default_project_rack_macros(),
                routing: crate::project::ProjectRackRouting::Broadcast,
                slots: vec![crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode:
                        crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: crate::project::ProjectEffectSlot::default(),
                    effect_slots: Vec::new(),
                    custom_effects: Vec::new(),
                    track_sound_state: crate::project::ProjectTrackSoundState::default(),
                    sample_path: Some(sample_path.to_string_lossy().into_owned()),
                    sample_name: Some("rack sample".to_string()),
                }],
            },
        };
        let sound_path = std::env::temp_dir().join(format!(
            "eseq-sound-rack-over-custom-history-{}.sound",
            std::process::id(),
        ));
        std::fs::write(
            &sound_path,
            serde_json::to_string(&sound).expect("serialize Sound"),
        ).expect("write Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());

        app.load_sound_onto_track(0, &sound_path)
            .expect("Sound rack should replace the custom instrument");
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(engine_id));
        assert_eq!(app.tracks[0], original_name);
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        assert_eq!(
            app.graph.engine_node_ids[engine_id]
                .as_ref()
                .expect("retained custom engine should be rebuilt")
                .route_gain_ids[0]
                .len(),
            MAX_VOICES,
        );

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .expect("Sound rack should be restored")
                .slots
                .len(),
            1,
        );
        graph.process_block();
    }

    #[test]
    fn replacing_rack_with_saved_instrument_undoes_and_redoes() {
        let graph = TestLiveGraph::new("rack-to-saved-instrument-history-test");
        let manifest = test_instrument_manifest();
        let mut app = test_app_with_track_count(&graph, 0);
        let sample_path = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample_path.to_path_buf()])
            .expect("rack should be created");
        let original_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let source = "target.lisp";
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "target".to_string(),
            source: source.to_string(),
            manifest,
            lib_index: 0,
            shared_runtime: true,
        });
        app.editor.instrument_libs.push(lisp_host::test_loaded_dgen_lib());

        app.try_swap_track_to_cached_saved_instrument_sync(
            0,
            "target",
            source,
            CustomInstrumentRunMode::Instrument,
        )
        .expect("cached instrument should be found")
        .expect("rack should accept a saved instrument");
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(engine_id));

        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        let restored_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("undo should restore the rack");
        assert_eq!(restored_rack.slots.len(), original_rack.slots.len());
        assert_eq!(restored_rack.slots[0].sample_id, original_rack.slots[0].sample_id);

        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(engine_id));
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        graph.process_block();
    }

    #[test]
    fn rack_to_sampler_conversion_keeps_rack_binding_when_voice_build_fails() {
        let graph = TestLiveGraph::new("rack-to-sampler-conversion-rollback-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack should be created");
        let before_nodes = app.graph.track_node_ids[0].rack_slots.clone();
        let before_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should be live");
        let buffer_id = crate::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should be created");
        set_test_graph_build_failure_after(4);

        let error = app.graph_controller()
            .replace_rack_track_with_sampler(0, buffer_id, 48_000, "restored")
            .expect_err("injected sampler voice failure should abort conversion");

        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(app.graph.track_node_ids[0].rack_slots.len(), before_nodes.len());
        for (after, before) in app.graph.track_node_ids[0].rack_slots.iter().zip(&before_nodes) {
            assert_eq!(after.sampler_ids, before.sampler_ids);
            assert_eq!(after.sampler_gatepitch_ids, before.sampler_gatepitch_ids);
            assert_eq!(after.sampler_modulator_ids, before.sampler_modulator_ids);
            assert_eq!(after.slot_sum_l_id, before.slot_sum_l_id);
            assert_eq!(after.slot_sum_r_id, before.slot_sum_r_id);
            assert_eq!(after.slot_pan_id, before.slot_pan_id);
        }
        let after_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should remain live");
        assert_eq!(after_rack.slots.len(), before_rack.slots.len());
        assert_eq!(
            after_rack.slots[0].sample_id,
            before_rack.slots[0].sample_id,
        );
        let created_nodes = take_test_graph_build_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            take_test_graph_build_rollback_node_ids(),
            created_nodes.iter().rev().copied().collect::<Vec<_>>(),
        );
        graph.process_block();
    }

    #[test]
    fn sampler_to_custom_conversion_keeps_sampler_binding_when_engine_build_fails() {
        let graph = TestLiveGraph::new("sampler-to-custom-conversion-rollback-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        let old_nodes = app.graph.track_node_ids[0].clone();
        let old_slot = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        let old_buffer_id = app.graph.track_buffer_ids[0];
        set_test_graph_build_failure_after(4);

        let error = app
            .graph_controller()
            .replace_track_with_custom_instrument(
                0,
                "new",
                0,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect_err("injected engine failure should abort sampler conversion");

        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Sampler]
        );
        assert_eq!(app.graph.track_engine_ids, vec![None]);
        assert_eq!(app.graph.track_buffer_ids, vec![old_buffer_id]);
        assert_eq!(
            app.graph.track_node_ids[0].sampler_ids,
            old_nodes.sampler_ids
        );
        assert_eq!(
            app.graph.track_node_ids[0].sampler_gatepitch_ids,
            old_nodes.sampler_gatepitch_ids
        );
        assert_eq!(
            app.graph.track_node_ids[0].sampler_modulator_ids,
            old_nodes.sampler_modulator_ids
        );
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]),
            &old_slot,
        );
        assert!(app.graph.engine_node_ids[0].is_none());
        let created_nodes = take_test_graph_build_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            take_test_graph_build_rollback_node_ids(),
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        graph.process_block();
    }

    #[test]
    fn custom_track_converts_to_sampler_without_replacing_its_shell() {
        let graph = TestLiveGraph::new("custom-to-sampler-conversion-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let voice_sum_id = app.graph.track_node_ids[0].voice_sum_id;
        let voice_sum_r_id = app.graph.track_node_ids[0].voice_sum_r_id;
        let buffer_id = crate::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should be created");

        let summary = app
            .graph_controller()
            .convert_custom_track_to_sampler(0, buffer_id, 48_000, "snare")
            .expect("custom track should convert to a sampler");

        assert_eq!(summary.patterns_reset, 1);
        assert_eq!(app.tracks, vec!["snare".to_string()]);
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Sampler]
        );
        assert_eq!(app.graph.track_engine_ids, vec![None]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, voice_sum_id);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_r_id, voice_sum_r_id);
        assert_eq!(app.graph.track_node_ids[0].sampler_ids.len(), MAX_VOICES);
        assert_eq!(app.graph.track_voice_lids[0].len(), MAX_VOICES);
        assert_eq!(app.graph.track_buffer_ids[0], buffer_id);
        assert_eq!(app.graph.track_sample_rates[0], 48_000);
        assert!(app.graph.engine_node_ids[0].is_none());
        assert_eq!(
            app.state.runtime.voice_counts[0].load(Ordering::Acquire),
            MAX_VOICES as u32
        );
        assert_eq!(
            app.state.export_pattern_repository()[0].sample_ids[0],
            (buffer_id, "snare".to_string(), 48_000)
        );

        let sample_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/ir/lexicon-300-rich-plate.wav");
        app.sampler_paths.push(Some(sample_path.clone()));
        app.register_loaded_sample_path("snare", buffer_id, sample_path.clone());
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let project_name = format!(
            "__test-custom-to-sampler-roundtrip-{}-{nonce}",
            std::process::id()
        );
        let captured = app
            .capture_project(&project_name)
            .expect("converted sampler project should capture");
        let project_path = crate::project::save_project(&project_name, &captured)
            .expect("converted sampler project should save");
        let _cleanup = TestProjectFile(project_path);
        let restored = crate::project::load_project(&project_name)
            .expect("converted sampler project should load");
        assert!(matches!(
            restored.tracks.as_slice(),
            [crate::project::ProjectTrack {
                kind: crate::project::ProjectTrackKind::Sampler { sample_path: restored_path },
                ..
            }] if restored_path == &sample_path.to_string_lossy()
        ));
        graph.process_block();
    }

    #[test]
    fn custom_to_sampler_conversion_keeps_custom_binding_when_voice_build_fails() {
        let graph = TestLiveGraph::new("custom-to-sampler-conversion-rollback-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let old_slot = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        let old_sum = app.graph.track_node_ids[0].voice_sum_id;
        let buffer_id = crate::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should be created");
        set_test_graph_build_failure_after(4);

        let error = app
            .graph_controller()
            .convert_custom_track_to_sampler(0, buffer_id, 48_000, "snare")
            .expect_err("injected sampler voice failure should abort conversion");

        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Custom]
        );
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, old_sum);
        assert!(app.graph.track_node_ids[0].sampler_ids.is_empty());
        assert_eq!(app.graph.track_buffer_ids, vec![-1]);
        assert_eq!(app.state.runtime.voice_counts[0].load(Ordering::Acquire), 0);
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]),
            &old_slot,
        );
        let old_engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("old custom engine should remain live");
        assert_eq!(old_engine.route_gain_ids[0].len(), MAX_VOICES);
        let created_nodes = take_test_graph_build_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            take_test_graph_build_rollback_node_ids(),
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        graph.process_block();
    }

    #[test]
    fn project_roundtrip_persists_swapped_custom_instrument() {
        let graph = TestLiveGraph::new("custom-track-swap-project-roundtrip-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        for (expected_id, name) in ["old", "new"].into_iter().enumerate() {
            let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
                name: name.to_string(),
                source: format!("{name}.lisp"),
                manifest: manifest.clone(),
                lib_index: 0,
                shared_runtime: true,
            });
            assert_eq!(engine_id, expected_id);
        }
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        app.editor
            .instrument_libs
            .push(lisp_host::test_loaded_dgen_lib());

        app.apply_recorded_instrument_binding_mutation(0, "Replace instrument", |app| {
            app.graph_controller().swap_custom_track_instrument(
                0, "new", 1, &manifest, &lib, CustomInstrumentRunMode::Instrument,
            )
        })
            .expect("track should swap before saving");
        assert_eq!(app.graph.track_engine_ids, vec![Some(1)]);

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        assert!(matches!(
            crate::tui::edit::undo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let undo_project_name = format!(
            "__test-instrument-swap-undo-roundtrip-{}-{nonce}",
            std::process::id()
        );
        let undo_captured = app
            .capture_project(&undo_project_name)
            .expect("undone project should capture");
        let undo_project_path = crate::project::save_project(&undo_project_name, &undo_captured)
            .expect("undone project should save");
        let _undo_cleanup = TestProjectFile(undo_project_path);
        let undo_restored = crate::project::load_project(&undo_project_name)
            .expect("undone project should load");
        assert!(matches!(
            undo_restored.tracks.as_slice(),
            [crate::project::ProjectTrack {
                kind: crate::project::ProjectTrackKind::Custom { instrument_name },
                ..
            }] if instrument_name == "old"
        ));
        assert!(matches!(
            crate::tui::edit::redo(&mut app),
            crate::tui::history::HistoryReplay::Applied(_)
        ));
        let project_name = format!(
            "__test-instrument-swap-roundtrip-{}-{nonce}",
            std::process::id()
        );
        let captured = app
            .capture_project(&project_name)
            .expect("swapped project should capture");
        let project_path = crate::project::save_project(&project_name, &captured)
            .expect("swapped project should save");
        let _cleanup = TestProjectFile(project_path);
        let restored =
            crate::project::load_project(&project_name).expect("swapped project should load");

        assert!(matches!(
            restored.tracks.as_slice(),
            [crate::project::ProjectTrack {
                kind: crate::project::ProjectTrackKind::Custom { instrument_name },
                ..
            }] if instrument_name == "new"
        ));
        graph.process_block();
    }

    #[test]
    fn swap_custom_track_leaves_old_binding_intact_when_new_engine_build_fails() {
        let graph = TestLiveGraph::new("custom-track-swap-failure-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let macro_id = app
            .macro_engine
            .create_macro("failed swap", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                macro_id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::InstrumentParam {
                        param: "old tone".to_string(),
                        param_id: None,
                    },
                    Some(0),
                    0.0,
                    1.0,
                    MacroCurve::Linear,
                )
                .expect("instrument mapping"),
            )
            .expect("instrument mapping should attach");
        let old_slot = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        set_test_graph_build_failure_after(4);

        let error = app
            .graph_controller()
            .swap_custom_track_instrument(
                0,
                "new",
                1,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect_err("injected engine build failure should abort the swap");
        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(
            app.state.runtime.track_engine_ids[0].load(Ordering::Acquire),
            0
        );
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]),
            &old_slot,
        );
        assert_eq!(
            app.macro_engine
                .macro_definition(macro_id)
                .expect("failed swap must preserve the macro")
                .mappings
                .len(),
            1,
            "failed swap must preserve the old instrument mapping"
        );
        let old_engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("old engine runtime must remain live");
        assert_eq!(old_engine.route_gain_ids[0].len(), MAX_VOICES);
        assert!(app.graph.engine_node_ids[1].is_none());
        graph.process_block();
    }

    #[test]
    fn connect_engine_to_track_rolls_back_every_graph_edit_before_publication() {
        let graph = TestLiveGraph::new("engine-route-rollback-test");
        let mut app = test_app(&graph);
        let targets = install_test_engine(&mut app, &graph);
        set_test_graph_build_failure_after(3);

        let error = {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            connect_test_engine(&mut app, &targets)
                .expect_err("injected route construction failure should be returned")
        };
        assert!(error.contains("injected graph node allocation failure"));

        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("test engine should remain registered");
        assert!(engine.route_gain_ids[0].is_empty());
        assert!(engine.ext_route_gain_ids[0].is_empty());
        for voice in 0..MAX_VOICES {
            assert_eq!(
                app.state.runtime.engine_route_lids[0][voice][0].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                app.state.runtime.engine_route_lids_r[0][voice][0].load(Ordering::Acquire),
                0
            );
            for input in 0..EXT_MOD_INPUT_COUNT {
                assert_eq!(
                    app.state.runtime.engine_ext_route_lids[0][voice][0][input]
                        .load(Ordering::Acquire),
                    0
                );
            }
        }

        let created_nodes = take_test_graph_build_node_ids();
        let rolled_back_nodes = take_test_graph_build_rollback_node_ids();
        assert_eq!(created_nodes.len(), 3);
        assert_eq!(
            rolled_back_nodes,
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        let created_connections = take_test_graph_build_connections();
        let rolled_back_connections = take_test_graph_build_rollback_connections();
        assert!(!created_connections.is_empty());
        assert_eq!(
            rolled_back_connections,
            created_connections
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>()
        );

        for &node_id in &created_nodes {
            assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, node_id) });
        }
        let probe_id = graph.add_gain(0.75, "post_rollback_probe");
        assert!(
            probe_id > *created_nodes.iter().max().unwrap(),
            "compensating rollback must not reuse logical IDs that were already queued"
        );
        assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, probe_id) });
        let mut observed_probe_gain = None;
        for _ in 0..4 {
            graph.process_block();
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            let copied = unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    probe_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            };
            if copied {
                assert_eq!(state_size, std::mem::size_of_val(&state));
                observed_probe_gain = Some(state[0]);
                break;
            }
        }
        assert_eq!(observed_probe_gain, Some(0.75));
        for node_id in created_nodes {
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            let copied = unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    node_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            };
            assert!(!copied, "rolled-back node {node_id} must not remain live");
            assert_eq!(state_size, 0);
        }
    }

    #[test]
    fn connect_engine_to_track_commits_complete_voice_routes_at_once() {
        let graph = TestLiveGraph::new("engine-route-commit-test");
        let mut app = test_app(&graph);
        let targets = install_test_engine(&mut app, &graph);
        begin_test_graph_build_capture();

        {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            connect_test_engine(&mut app, &targets).expect("complete route build should succeed");
        }

        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("test engine should remain registered");
        assert_eq!(engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(engine.ext_route_gain_ids[0].len(), MAX_VOICES);
        for voice in 0..MAX_VOICES {
            let [left_id, right_id] = engine.route_gain_ids[0][voice];
            assert!(left_id > 0);
            assert!(right_id > 0);
            assert_eq!(
                app.state.runtime.engine_route_lids[0][voice][0].load(Ordering::Acquire),
                left_id as u64
            );
            assert_eq!(
                app.state.runtime.engine_route_lids_r[0][voice][0].load(Ordering::Acquire),
                right_id as u64
            );
            for input in 0..EXT_MOD_INPUT_COUNT {
                let ext_id = engine.ext_route_gain_ids[0][voice][input];
                assert!(ext_id > 0);
                assert_eq!(
                    app.state.runtime.engine_ext_route_lids[0][voice][0][input]
                        .load(Ordering::Acquire),
                    ext_id as u64
                );
            }
        }

        assert_eq!(
            take_test_graph_build_node_ids().len(),
            MAX_VOICES * (2 + EXT_MOD_INPUT_COUNT)
        );
        assert!(!take_test_graph_build_connections().is_empty());
        assert!(take_test_graph_build_rollback_node_ids().is_empty());
        assert!(take_test_graph_build_rollback_connections().is_empty());
        graph.process_block();
    }
}
