use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::effects::EffectDescriptor;
use crate::lisp_host::{self, DGenManifest, LoadedDGenLib};
use crate::sequencer::{
    BusId, CustomInstrumentRunMode, InstrumentType, ModDestination, TrackOutput,
    EXT_MOD_INPUT_COUNT, MAX_TRACKS,
};
use crate::voice::MAX_VOICES;

use super::{App, EngineDescriptor, EngineNodeIds, TrackNodeIds};

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

fn instrument_display_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
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

pub struct GraphController<'a> {
    app: &'a mut App,
}

struct GraphEditBatchGuard {
    lg: *mut crate::audiograph::LiveGraph,
}

impl GraphEditBatchGuard {
    fn new(lg: *mut crate::audiograph::LiveGraph) -> Self {
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        Self { lg }
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
        "gate" => Some(crate::gatepitch::PARAM_GATE as i32),
        "pitch" => Some(crate::gatepitch::PARAM_PITCH as i32),
        "velocity" | "vel" => Some(crate::gatepitch::PARAM_VELOCITY as i32),
        "trigger" | "trig" => Some(crate::gatepitch::PARAM_TRIGGER as i32),
        "clock" | "barclock" => Some(crate::gatepitch::PARAM_CLOCK_PHASE as i32),
        _ if manifest_lacks_names && input.channel < 4 => Some(input.channel as i32),
        _ => None,
    }
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
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
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
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
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
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
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
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

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
        });
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
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

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
        });
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_modulator_track(&mut self) -> Result<usize, String> {
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        let track_name = format!("Modulator {}", idx + 1);
        let shell = self.create_track_shell(idx, &track_name)?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: Vec::new(),
            instrument: InstrumentRegistration::Modulator,
        });
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
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        let track_name = instrument_display_name(name);
        let shell = self.create_track_shell(idx, &track_name)?;
        self.ensure_custom_engine_runtime(engine_id, name, manifest, lib)?;
        self.connect_engine_to_track(
            engine_id,
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
        });
        self.app.sampler_paths.push(None);
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(idx)?;
        }
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
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
    }

    pub fn clear_all_tracks(&mut self) {
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let old_track_count = self.app.tracks.len();

        for track_idx in 0..old_track_count {
            for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
                ..self.app.state.pattern.effect_chains[track_idx].len()
            {
                let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
                let node_id = slot.node_id.load(Ordering::Relaxed);
                let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
                if node_id == 0 {
                    continue;
                }
                let offset = slot_idx - crate::effects::BUILTIN_SLOT_COUNT;
                let predecessor_id = self.find_custom_slot_predecessor(track_idx, offset);
                let successor_id = self.find_custom_slot_successor(track_idx, offset);
                unsafe {
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        0,
                        node_id as i32,
                        0,
                    );
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        node_id as i32,
                        0,
                        successor_id,
                        0,
                    );
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        0,
                        successor_id,
                        0,
                    );
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
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
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

        let names = self.app.tracks.clone();
        let buffer_ids = self.app.graph.track_buffer_ids.clone();
        let sample_rates = self.app.graph.track_sample_rates.clone();
        let instrument_types = self.app.graph.track_instrument_types.clone();
        let deleted_engine_id = self.app.graph.track_engine_ids[track_idx];

        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            self.delete_custom_effect_chain(track_idx);
            self.delete_track_engine_routes(track_idx);

            let track_nodes = self.app.graph.track_node_ids[track_idx].clone();
            self.delete_track_shell(&track_nodes);

            if let Some(engine_id) = deleted_engine_id {
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

        self.compact_app_track_vectors(track_idx);
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

        if self.app.is_sampler_track(track_idx) {
            self.send_sample_to_all_voices(track_idx, -1, self.app.graph.sample_rate);
        }

        for engine_id in 0..MAX_TRACKS {
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

    fn find_custom_slot_predecessor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.app.state.pattern.effect_chains[track];
        for i in (0..offset).rev() {
            let idx = crate::effects::BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let node_id = chain[idx].node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    return node_id as i32;
                }
            }
        }
        self.app.graph.track_node_ids[track].pan_id
    }

    fn find_custom_slot_successor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.app.state.pattern.effect_chains[track];
        for i in (offset + 1)..crate::lisp_host::MAX_CUSTOM_FX {
            let idx = crate::effects::BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let node_id = chain[idx].node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    return node_id as i32;
                }
            }
        }
        self.app.graph.track_node_ids[track].delay_id
    }

    fn find_custom_slot_predecessor_with_channels(
        &self,
        track: usize,
        offset: usize,
    ) -> (i32, usize) {
        let chain = &self.app.state.pattern.effect_chains[track];
        for i in (0..offset).rev() {
            let idx = crate::effects::BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let node_id = chain[idx].node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    let channels = self.app.graph.effect_descriptors[track][idx]
                        .output_channels
                        .max(1);
                    return (node_id as i32, channels);
                }
            }
        }
        (self.app.graph.track_node_ids[track].pan_id, 2)
    }

    fn find_custom_slot_successor_with_channels(
        &self,
        track: usize,
        offset: usize,
    ) -> (i32, usize) {
        let chain = &self.app.state.pattern.effect_chains[track];
        for i in (offset + 1)..crate::lisp_host::MAX_CUSTOM_FX {
            let idx = crate::effects::BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let node_id = chain[idx].node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    let channels = self.app.graph.effect_descriptors[track][idx]
                        .input_channels
                        .max(1);
                    return (node_id as i32, channels);
                }
            }
        }
        (self.app.graph.track_node_ids[track].delay_id, 2)
    }

    fn connect_custom_effect_gap(
        &self,
        predecessor_id: i32,
        predecessor_outputs: usize,
        successor_id: i32,
        successor_inputs: usize,
    ) {
        let predecessor_channels = predecessor_outputs.max(1).min(2);
        let successor_channels = successor_inputs.max(1).min(2);
        unsafe {
            if predecessor_channels <= 1 {
                for dst_port in 0..successor_channels {
                    let _ = crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        0,
                        successor_id,
                        dst_port as i32,
                    );
                }
            } else if successor_channels <= 1 {
                for src_port in 0..predecessor_channels {
                    let _ = crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        src_port as i32,
                        successor_id,
                        0,
                    );
                }
            } else {
                for ch in 0..predecessor_channels.min(successor_channels) {
                    let _ = crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        ch as i32,
                        successor_id,
                        ch as i32,
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
        let modulator_node_id = self.app.state.pattern.effect_chains[track_idx][slot_idx]
            .modulator_node_id
            .load(Ordering::Relaxed);
        if node_id == 0 {
            return Err("Effect slot is empty".to_string());
        }

        let offset = slot_idx - crate::effects::BUILTIN_SLOT_COUNT;
        let (predecessor_id, predecessor_outputs) =
            self.find_custom_slot_predecessor_with_channels(track_idx, offset);
        let (successor_id, successor_inputs) =
            self.find_custom_slot_successor_with_channels(track_idx, offset);

        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            unsafe {
                crate::lisp_host::remove_effect_from_chain(
                    self.app.graph.lg.0,
                    node_id as i32,
                    predecessor_id,
                    successor_id,
                );
                crate::lisp_host::remove_effect_modulator(
                    self.app.graph.lg.0,
                    modulator_node_id as i32,
                );
            }
            self.connect_custom_effect_gap(
                predecessor_id,
                predecessor_outputs,
                successor_id,
                successor_inputs,
            );
        }

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

    fn delete_custom_effect_chain(&mut self, track_idx: usize) {
        for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
            ..self.app.state.pattern.effect_chains[track_idx].len()
        {
            let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
            let node_id = slot.node_id.load(Ordering::Relaxed);
            let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            let offset = slot_idx - crate::effects::BUILTIN_SLOT_COUNT;
            let predecessor_id = self.find_custom_slot_predecessor(track_idx, offset);
            let successor_id = self.find_custom_slot_successor(track_idx, offset);
            unsafe {
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    predecessor_id,
                    0,
                    node_id as i32,
                    0,
                );
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    node_id as i32,
                    0,
                    successor_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    predecessor_id,
                    0,
                    successor_id,
                    0,
                );
                crate::audiograph::delete_node(self.app.graph.lg.0, node_id as i32);
                if modulator_node_id != 0 {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_node_id as i32);
                }
            }
        }
    }

    fn delete_track_engine_routes(&mut self, track_idx: usize) {
        for (engine_id, engine) in self.app.graph.engine_node_ids.iter_mut().enumerate() {
            let Some(engine) = engine.as_mut() else {
                continue;
            };
            if track_idx >= engine.route_gain_ids.len() {
                continue;
            }
            for route_pair in &engine.route_gain_ids[track_idx] {
                for &route_id in route_pair {
                    if route_id <= 0 {
                        continue;
                    }
                    unsafe {
                        crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                    }
                }
            }
            engine.route_gain_ids[track_idx].clear();
            for voice in 0..MAX_VOICES {
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
            if track_idx < engine.ext_route_gain_ids.len() {
                for route_ids in &engine.ext_route_gain_ids[track_idx] {
                    for &route_id in route_ids {
                        if route_id > 0 {
                            unsafe {
                                crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                            }
                        }
                    }
                }
                engine.ext_route_gain_ids[track_idx].clear();
            }
        }
    }

    fn delete_track_shell(&mut self, track: &TrackNodeIds) {
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

    fn engine_is_still_referenced_excluding(&self, engine_id: usize, removed_track: usize) -> bool {
        self.app
            .graph
            .track_engine_ids
            .iter()
            .enumerate()
            .any(|(track_idx, binding)| track_idx != removed_track && *binding == Some(engine_id))
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
        lisp_host::reset_dgen_engine_enabled_voices(engine_id);
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
        }
    }

    fn compact_app_track_vectors(&mut self, track_idx: usize) {
        self.app.tracks.remove(track_idx);
        if track_idx < self.app.track_colors.len() {
            self.app.track_colors.remove(track_idx);
        }
        if track_idx < self.app.track_collapsed.len() {
            self.app.track_collapsed.remove(track_idx);
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
            crate::gatepitch::PARAM_TRIGGER,
            0.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::gatepitch::PARAM_PITCH,
            440.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::gatepitch::PARAM_VELOCITY,
            1.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::gatepitch::PARAM_GATE,
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
    }

    fn create_track_shell(&mut self, idx: usize, name: &str) -> Result<TrackShell, String> {
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
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
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
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
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

    fn build_sampler_voices(
        &mut self,
        track_idx: usize,
        track_name: &str,
        buffer_id: i32,
        sample_rate: u32,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    ) -> Result<SamplerVoiceSetup, String> {
        let mut sampler_ids = Vec::with_capacity(MAX_VOICES);
        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut modulator_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let gp_name = CString::new(format!("{}_gp_{}", track_name, v)).unwrap();
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::gatepitch::gatepitch_vtable(),
                    crate::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    crate::gatepitch::OUTPUT_COUNT as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!(
                    "build_sampler_voices: failed to add gatepitch node for voice {v}"
                ));
            }

            let mod_name = CString::new(format!("{}_mod_{}", track_name, v)).unwrap();
            let mod_initial_state =
                crate::voice_modulator::sampler_voice_initial_state(track_idx, v);
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
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.app.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::voice_modulator::PARAM_BPM as u64,
                        logical_id: mod_id as u64,
                        fvalue: self.app.state.transport.bpm.load(Ordering::Relaxed) as f32,
                    },
                );
            }
            let node_name = format!("{}_{}", track_name, v);
            let st = crate::sampler::create_sampler_node(
                self.app.graph.lg.0,
                buffer_id,
                sample_rate,
                &node_name,
            )?;
            unsafe {
                for port in 0..4 {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        gp_id,
                        port,
                        mod_id,
                        port,
                    );
                }
                for port in 0..crate::voice_modulator::NUM_OUTPUTS {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        mod_id,
                        port as i32,
                        st.node_id,
                        port as i32,
                    );
                }
                for (input, &clip_id) in track_mod_in_clip_ids.iter().enumerate() {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        clip_id,
                        0,
                        mod_id,
                        (4 + input) as i32,
                    );
                }
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    st.node_id,
                    0,
                    voice_sum_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    st.node_id,
                    1,
                    voice_sum_r_id,
                    0,
                );
            }
            sampler_ids.push(st.node_id);
            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            voice_lids.push(st.logical_id);
        }

        Ok(SamplerVoiceSetup {
            sampler_ids,
            gatepitch_ids,
            modulator_ids,
            voice_lids,
        })
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
        manifest: &DGenManifest,
        context: &str,
    ) -> Result<(), String> {
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
            let Some(host_output) = host_signal_output_for_input(manifest, input) else {
                continue;
            };
            self.graph_connect_checked(
                gp_id,
                host_output,
                synth_id,
                input.channel as i32,
                &format!(
                    "{context} host input '{}' channel {}",
                    input.name, input.channel
                ),
            )?;
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
            self.graph_connect_checked(
                mod_id,
                (modulator.slot - 1) as i32,
                synth_id,
                modulator.input_channel as i32,
                &format!(
                    "{context} modulator '{}' slot {} channel {}",
                    modulator.name, modulator.slot, modulator.input_channel
                ),
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

        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut synth_ids = Vec::with_capacity(MAX_VOICES);
        let mut modulator_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let gp_name = CString::new(format!("{}_gp_{}", name, v)).unwrap();
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::gatepitch::gatepitch_vtable(),
                    crate::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    crate::gatepitch::OUTPUT_COUNT as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!(
                    "ensure_custom_engine_runtime: failed to add gatepitch node for engine {} voice {}",
                    engine_id, v
                ));
            }

            let mod_name = CString::new(format!("{}_mod_{}", name, v)).unwrap();
            let mod_initial_state =
                crate::voice_modulator::custom_engine_initial_state(engine_id, v);
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
                    "ensure_custom_engine_runtime: failed to add modulator node for engine {} voice {}",
                    engine_id, v
                ));
            }
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.app.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::voice_modulator::PARAM_BPM as u64,
                        logical_id: mod_id as u64,
                        fvalue: self.app.state.transport.bpm.load(Ordering::Relaxed) as f32,
                    },
                );
            }

            let slot_id = engine_id * MAX_VOICES + v;
            lisp_host::set_dgen_instrument_fn(slot_id, lib.process_fn);
            lisp_host::set_dgen_instrument_output_count(slot_id, manifest.n_outputs.max(1));
            let init_msg = lisp_host::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_host::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            let synth_name = CString::new(format!("{}_engine_synth_{}", name, v)).unwrap();
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
                    "ensure_custom_engine_runtime: failed to add synth node for engine {} voice {} (manifest.n_inputs={})",
                    engine_id, v, manifest.n_inputs
                ));
            }
            self.connect_custom_host_inputs(
                gp_id,
                mod_id,
                synth_id,
                manifest,
                &format!("ensure_custom_engine_runtime engine {engine_id} voice {v}"),
            )?;
            self.graph_connect_checked(
                gp_id,
                0,
                mod_id,
                0,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            self.graph_connect_checked(
                gp_id,
                1,
                mod_id,
                1,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            self.graph_connect_checked(
                gp_id,
                2,
                mod_id,
                2,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            self.graph_connect_checked(
                gp_id,
                3,
                mod_id,
                3,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            synth_ids.push(synth_id);
            voice_lids.push(gp_id as u64);
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
            route_gain_ids: (0..MAX_TRACKS).map(|_| Vec::new()).collect(),
            ext_route_gain_ids: (0..MAX_TRACKS).map(|_| Vec::new()).collect(),
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
        track_idx: usize,
        track_name: &str,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_out_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        let Some(existing_engine) = self.app.graph.engine_node_ids[engine_id].as_ref() else {
            return Err(format!(
                "connect_engine_to_track: missing engine runtime for engine {}",
                engine_id
            ));
        };
        if existing_engine.route_gain_ids[track_idx].len() == MAX_VOICES {
            return Ok(());
        }
        let synth_ids = existing_engine.synth_ids.clone();
        let audio_output_channels = existing_engine.audio_output_channels.clone();
        let primary_mod_output_channel = existing_engine.mod_output_channels.first().copied();
        let modulator_ids = existing_engine.modulator_ids.clone();

        let mut route_ids = Vec::with_capacity(MAX_VOICES);
        let mut ext_route_ids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let route_l_id = add_gain_node_checked(
                self.app.graph.lg.0,
                0.0,
                &format!("{}_eng{}_route_{}_l", track_name, engine_id, v),
                &format!(
                    "connect_engine_to_track left route engine {} track {} voice {}",
                    engine_id, track_idx, v
                ),
            )?;
            if let Some(src_channel) = stereo_route_source_channel(&audio_output_channels, 0) {
                self.graph_connect_checked(
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
            self.graph_connect_checked(
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
            self.app.state.runtime.engine_route_lids[engine_id][v][track_idx]
                .store(route_l_id as u64, Ordering::Release);

            if let Some(src_channel) = stereo_route_source_channel(&audio_output_channels, 1) {
                let route_r_id = add_gain_node_checked(
                    self.app.graph.lg.0,
                    0.0,
                    &format!("{}_eng{}_route_{}_r", track_name, engine_id, v),
                    &format!(
                        "connect_engine_to_track right route engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.graph_connect_checked(
                    synth_ids[v],
                    src_channel as i32,
                    route_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.graph_connect_checked(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.app.state.runtime.engine_route_lids_r[engine_id][v][track_idx]
                    .store(route_r_id as u64, Ordering::Release);
                route_pair[1] = route_r_id;
            } else {
                let route_r_id = add_gain_node_checked(
                    self.app.graph.lg.0,
                    0.0,
                    &format!("{}_eng{}_route_{}_r", track_name, engine_id, v),
                    &format!(
                        "connect_engine_to_track mirrored right route engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.graph_connect_checked(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track mirrored-right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.app.state.runtime.engine_route_lids_r[engine_id][v][track_idx]
                    .store(route_r_id as u64, Ordering::Release);
                route_pair[1] = route_r_id;
            }

            route_ids.push(route_pair);

            if let Some(src_channel) = primary_mod_output_channel {
                self.graph_connect_checked(
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

            let mut voice_ext_route_ids = [0; EXT_MOD_INPUT_COUNT];
            for input in 0..EXT_MOD_INPUT_COUNT {
                if !modulator_ids.is_empty() {
                    let ext_route_id = add_gain_node_checked(
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
                    )?;
                    self.graph_connect_checked(
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
                    self.graph_connect_checked(
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
                    self.app.state.runtime.engine_ext_route_lids[engine_id][v][track_idx][input]
                        .store(ext_route_id as u64, Ordering::Release);
                    voice_ext_route_ids[input] = ext_route_id;
                } else {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][v][track_idx][input]
                        .store(0, Ordering::Release);
                }
            }
            ext_route_ids.push(voice_ext_route_ids);
        }

        let Some(engine) = self.app.graph.engine_node_ids[engine_id].as_mut() else {
            return Err(format!(
                "connect_engine_to_track: engine runtime disappeared for engine {}",
                engine_id
            ));
        };
        engine.route_gain_ids[track_idx] = route_ids;
        engine.ext_route_gain_ids[track_idx] = ext_route_ids;
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
                            idx: crate::gatepitch::PARAM_GATE,
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
                manifest,
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

    fn finish_track_registration(&mut self, registration: TrackRegistration<'_>) {
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
            InstrumentType::Sampler | InstrumentType::Modulator => super::SidebarMode::Audition,
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
    }

    fn debug_assert_track_vectors_aligned(&self) {
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
    use super::free_patch_idle_route_value;

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
}
