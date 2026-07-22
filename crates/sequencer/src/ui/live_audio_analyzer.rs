use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CString};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use eseqlisp::live_audio::{BandMeterFrame, CompressorMeterFrame};
use eseqlisp::widget_render::compressor_display::{
    collect_compressor_meter_requests, CompressorMeterRequest,
};
use eseqlisp::widget_render::live_audio::{LiveAudioSourceSelector, TapPoint};
use eseqlisp::widget_render::multiband_meter::{collect_band_meter_requests, BandMeterRequest};
use eseqlisp::widget_render::roar_shaper::collect_roar_meter_requests;
use eseqlisp::widget_render::scope::{collect_scope_requests, ScopeRequest};
use eseqlisp::widget_render::spectrogram::{collect_spectrogram_requests, SpectrogramRequest};
use sequencer::audio_tap::{self, SpectrogramProcessor};
use sequencer::audiograph::{self, LiveGraphPtr};
use sequencer::sequencer::BusId;
use sequencer::app;

use crate::constants::LIVE_AUDIO_ANALYZER_POLL_INTERVAL;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TapKey {
    source: LiveAudioSourceSelector,
    tap_point: TapPoint,
}

impl TapKey {
    fn from_request(request: &SpectrogramRequest) -> Self {
        Self {
            source: request.source.clone(),
            tap_point: request.tap_point,
        }
    }

    fn from_scope_request(request: &ScopeRequest) -> Self {
        Self {
            source: request.source.clone(),
            tap_point: request.tap_point,
        }
    }

    fn label(&self) -> String {
        format!(
            "audio_tap_{}_{}",
            self.source.key_fragment().replace(':', "_"),
            self.tap_point.key_fragment().replace('-', "_")
        )
    }
}

struct TapNode {
    node_id: i32,
    source_node: i32,
    ring_frames: usize,
    state: Vec<f32>,
}

/// One watched multiband-dynamics effect node whose state meters feed a
/// `multiband-meter` widget.
struct MeterNode {
    node_id: i32,
    revision: u64,
    last_frame: Option<BandMeterFrame>,
}

/// One watched Compressor effect node whose state meter ring feeds a
/// `compressor-display` widget.
struct CompressorMeterNode {
    node_id: i32,
    revision: u64,
    last_ring_write: f32,
}

pub(crate) struct LiveAudioAnalyzerManager {
    lg: LiveGraphPtr,
    taps: HashMap<TapKey, TapNode>,
    processors: HashMap<String, SpectrogramProcessor>,
    meter_nodes: HashMap<String, MeterNode>,
    compressor_nodes: HashMap<String, CompressorMeterNode>,
    last_poll_at: Instant,
}

impl LiveAudioAnalyzerManager {
    pub(crate) fn new(lg: LiveGraphPtr) -> Self {
        Self {
            lg,
            taps: HashMap::new(),
            processors: HashMap::new(),
            meter_nodes: HashMap::new(),
            compressor_nodes: HashMap::new(),
            last_poll_at: Instant::now() - LIVE_AUDIO_ANALYZER_POLL_INTERVAL,
        }
    }

    pub(crate) fn sync_visible(&mut self, editor: &eseqlisp::Editor, app: &app::App) -> bool {
        if app.has_pending_project_load() {
            return self.suspend_for_project_load();
        }

        let mut grouped: HashMap<TapKey, Vec<SpectrogramRequest>> = HashMap::new();
        let mut scope_grouped: HashMap<TapKey, Vec<ScopeRequest>> = HashMap::new();
        let mut meter_requests: HashMap<String, BandMeterRequest> = HashMap::new();
        let mut compressor_requests: HashMap<String, CompressorMeterRequest> = HashMap::new();
        for layout in editor.visible_widget_layouts() {
            for request in collect_spectrogram_requests(layout.as_ref()) {
                grouped
                    .entry(TapKey::from_request(&request))
                    .or_default()
                    .push(request);
            }
            for request in collect_scope_requests(layout.as_ref()) {
                scope_grouped
                    .entry(TapKey::from_scope_request(&request))
                    .or_default()
                    .push(request);
            }
            for request in collect_band_meter_requests(layout.as_ref()) {
                meter_requests
                    .entry(request.data_key.clone())
                    .or_insert(request);
            }
            // Roar shaper views read their effect node's state meters over
            // the same watchlist path, keyed `roar-meter:`.
            for request in collect_roar_meter_requests(layout.as_ref()) {
                meter_requests
                    .entry(request.data_key.clone())
                    .or_insert(BandMeterRequest {
                        data_key: request.data_key,
                        source: request.source,
                    });
            }
            for request in collect_compressor_meter_requests(layout.as_ref()) {
                compressor_requests
                    .entry(request.data_key.clone())
                    .or_insert(request);
            }
        }
        let poll_due = self.last_poll_at.elapsed() >= LIVE_AUDIO_ANALYZER_POLL_INTERVAL;
        let meters_changed = self.sync_band_meters(app, meter_requests, poll_due)
            | self.sync_compressor_meters(app, compressor_requests, poll_due);

        let mut active_keys = HashSet::new();
        let mut active_scope_keys = HashSet::new();
        let mut topology_changed = false;
        let mut active_tap_keys = HashSet::new();
        let requested_tap_keys = grouped
            .keys()
            .chain(scope_grouped.keys())
            .cloned()
            .collect::<HashSet<_>>();
        for tap_key in requested_tap_keys {
            let Some(source_node) = resolve_source_node(app, &tap_key) else {
                continue;
            };
            active_tap_keys.insert(tap_key.clone());
            for request in grouped.get(&tap_key).into_iter().flatten() {
                active_keys.insert(request.data_key.clone());
            }
            for request in scope_grouped.get(&tap_key).into_iter().flatten() {
                active_scope_keys.insert(request.data_key.clone());
            }
            let required_ring_frames = grouped
                .get(&tap_key)
                .into_iter()
                .flatten()
                .map(|request| request.required_ring_frames)
                .chain(
                    scope_grouped
                        .get(&tap_key)
                        .into_iter()
                        .flatten()
                        .map(|request| request.frame_count),
                )
                .max()
                .map(audio_tap::normalize_ring_frames)
                .unwrap_or(audio_tap::MIN_TAP_RING_FRAMES);

            let needs_recreate = self
                .taps
                .get(&tap_key)
                .map(|tap| tap.source_node != source_node || tap.ring_frames < required_ring_frames)
                .unwrap_or(true);
            if needs_recreate {
                if let Some(old_tap) = self.taps.remove(&tap_key) {
                    self.destroy_tap(old_tap);
                }
                match self.create_tap(&tap_key, source_node, required_ring_frames) {
                    Some(tap) => {
                        self.taps.insert(tap_key.clone(), tap);
                        topology_changed = true;
                    }
                    None => {
                        for request in grouped.get(&tap_key).into_iter().flatten() {
                            active_keys.remove(&request.data_key);
                        }
                        for request in scope_grouped.get(&tap_key).into_iter().flatten() {
                            active_scope_keys.remove(&request.data_key);
                        }
                        active_tap_keys.remove(&tap_key);
                    }
                }
            }
        }

        let stale_tap_keys = self
            .taps
            .keys()
            .filter(|tap_key| !active_tap_keys.contains(*tap_key))
            .cloned()
            .collect::<Vec<_>>();
        for tap_key in stale_tap_keys {
            if let Some(tap) = self.taps.remove(&tap_key) {
                self.destroy_tap(tap);
                topology_changed = true;
            }
        }

        self.processors
            .retain(|data_key, _| active_keys.contains(data_key));
        eseqlisp::live_audio::retain_spectrogram_frames(&active_keys);
        eseqlisp::live_audio::retain_scope_frames(&active_scope_keys);

        let mut published_frame = false;
        if self.last_poll_at.elapsed() >= LIVE_AUDIO_ANALYZER_POLL_INTERVAL {
            for tap_key in active_tap_keys.iter().cloned().collect::<Vec<_>>() {
                if !active_tap_keys.contains(&tap_key) {
                    continue;
                }
                let Some(tap) = self.taps.get_mut(&tap_key) else {
                    continue;
                };
                let mut state_size = 0usize;
                let copied = unsafe {
                    audiograph::get_node_state_into(
                        self.lg.0,
                        tap.node_id,
                        tap.state.as_mut_ptr() as *mut c_void,
                        tap.state.len() * std::mem::size_of::<f32>(),
                        &mut state_size as *mut usize,
                    )
                };
                if !copied || state_size > tap.state.len() * std::mem::size_of::<f32>() {
                    continue;
                }
                let floats = state_size / std::mem::size_of::<f32>();
                let state = &tap.state[..floats.min(tap.state.len())];
                let mut unique_requests = HashMap::<String, SpectrogramRequest>::new();
                for request in grouped.remove(&tap_key).unwrap_or_default() {
                    unique_requests
                        .entry(request.data_key.clone())
                        .or_insert(request);
                }
                for request in unique_requests.into_values() {
                    let processor = self
                        .processors
                        .entry(request.data_key.clone())
                        .or_insert_with(|| {
                            SpectrogramProcessor::new(
                                request.fft_size,
                                request.time_slices,
                                request.min_db,
                                request.max_db,
                                request.smoothing,
                            )
                        });
                    if processor.fft_size() != request.fft_size
                        || processor.time_slices() != request.time_slices
                    {
                        *processor = SpectrogramProcessor::new(
                            request.fft_size,
                            request.time_slices,
                            request.min_db,
                            request.max_db,
                            request.smoothing,
                        );
                    }
                    if let Some(frame) = processor.update_from_tap_state(state) {
                        eseqlisp::live_audio::publish_spectrogram_frame(
                            request.data_key,
                            eseqlisp::live_audio::SpectrogramFrame {
                                revision: frame.revision,
                                bins: frame.bins,
                                time_slices: frame.time_slices,
                                write_head: frame.write_head,
                                sample_rate: frame.sample_rate,
                                waterfall: Arc::new(frame.waterfall),
                                smoothed: Arc::new(frame.smoothed),
                            },
                        );
                        published_frame = true;
                    }
                }
                let metadata = audio_tap::tap_metadata(state);
                let mut unique_scope_requests = HashMap::<String, ScopeRequest>::new();
                for request in scope_grouped.remove(&tap_key).unwrap_or_default() {
                    unique_scope_requests
                        .entry(request.data_key.clone())
                        .or_insert(request);
                }
                for request in unique_scope_requests.into_values() {
                    let Some(samples) = audio_tap::read_latest_mono(state, request.frame_count)
                    else {
                        continue;
                    };
                    let Some(metadata) = metadata else {
                        continue;
                    };
                    eseqlisp::live_audio::publish_scope_frame(
                        request.data_key,
                        eseqlisp::live_audio::ScopeFrame {
                            revision: metadata.write_head as u64,
                            sample_rate: metadata.sample_rate,
                            samples: Arc::new(samples),
                        },
                    );
                    published_frame = true;
                }
            }
            self.last_poll_at = Instant::now();
        }

        topology_changed || published_frame || meters_changed
    }

    /// Watches the effect nodes behind visible `multiband-meter` widgets and
    /// republishes their state meters when the values move.
    fn sync_band_meters(
        &mut self,
        app: &app::App,
        requests: HashMap<String, BandMeterRequest>,
        poll_due: bool,
    ) -> bool {
        let mut changed = false;
        let stale_keys = self
            .meter_nodes
            .keys()
            .filter(|key| !requests.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale_keys {
            if let Some(node) = self.meter_nodes.remove(&key) {
                let _ = unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
            }
        }
        eseqlisp::live_audio::retain_band_meter_frames(&requests.keys().cloned().collect());

        for (data_key, request) in requests {
            let Some(node_id) = resolve_effect_node(app, &request.source) else {
                if let Some(node) = self.meter_nodes.remove(&data_key) {
                    let _ =
                        unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
                }
                continue;
            };
            let needs_watch = self
                .meter_nodes
                .get(&data_key)
                .map(|node| node.node_id != node_id)
                .unwrap_or(true);
            if needs_watch {
                if let Some(node) = self.meter_nodes.remove(&data_key) {
                    let _ =
                        unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
                }
                if !unsafe { audiograph::add_node_to_watchlist(self.lg.0, node_id) } {
                    continue;
                }
                self.meter_nodes.insert(
                    data_key.clone(),
                    MeterNode {
                        node_id,
                        revision: 0,
                        last_frame: None,
                    },
                );
            }
            if !poll_due {
                continue;
            }
            let Some(node) = self.meter_nodes.get_mut(&data_key) else {
                continue;
            };
            let is_roar = data_key.starts_with("roar-meter:");
            let state_len = if is_roar {
                sequencer::effects::roar::ROAR_STATE_SIZE
            } else {
                sequencer::effects::ott::OTT_STATE_SIZE
            };
            let mut state = vec![0.0f32; state_len];
            let state_bytes = state_len * std::mem::size_of::<f32>();
            let mut state_size = 0usize;
            let copied = unsafe {
                audiograph::get_node_state_into(
                    self.lg.0,
                    node.node_id,
                    state.as_mut_ptr().cast(),
                    state_bytes,
                    &mut state_size as *mut usize,
                )
            };
            if !copied || state_size < state_bytes {
                continue;
            }
            let mut frame = BandMeterFrame {
                revision: node.revision + 1,
                level_db: [[0.0; 2]; 3],
                gain_db: [0.0; 3],
            };
            if is_roar {
                // Per stage: (pre-shaper min, max) as the level pair (linear
                // curve-domain values) and the post-stage dB as the gain.
                for stage in 0..3 {
                    for slot in 0..2 {
                        frame.level_db[stage][slot] =
                            state[sequencer::effects::roar::STATE_METER_PRE + stage * 2 + slot];
                    }
                    frame.gain_db[stage] =
                        state[sequencer::effects::roar::STATE_METER_POST_DB + stage];
                }
            } else {
                for band in 0..3 {
                    for ch in 0..2 {
                        frame.level_db[band][ch] =
                            state[sequencer::effects::ott::STATE_METER_LEVEL_DB + band * 2 + ch];
                    }
                    frame.gain_db[band] =
                        state[sequencer::effects::ott::STATE_METER_GAIN_DB + band];
                }
            }
            let moved = node.last_frame.map_or(true, |last| {
                (0..3).any(|band| {
                    (frame.gain_db[band] - last.gain_db[band]).abs() > 0.05
                        || (0..2).any(|ch| {
                            (frame.level_db[band][ch] - last.level_db[band][ch]).abs() > 0.05
                        })
                })
            });
            if moved {
                node.revision += 1;
                node.last_frame = Some(frame);
                eseqlisp::live_audio::publish_band_meter_frame(data_key, frame);
                changed = true;
            }
        }
        changed
    }

    /// Watches the Compressor effect nodes behind visible
    /// `compressor-display` widgets and republishes their meter rings.
    fn sync_compressor_meters(
        &mut self,
        app: &app::App,
        requests: HashMap<String, CompressorMeterRequest>,
        poll_due: bool,
    ) -> bool {
        use sequencer::effects::compressor as comp;

        let mut changed = false;
        let stale_keys = self
            .compressor_nodes
            .keys()
            .filter(|key| !requests.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale_keys {
            if let Some(node) = self.compressor_nodes.remove(&key) {
                let _ = unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
            }
        }
        eseqlisp::live_audio::retain_compressor_meter_frames(&requests.keys().cloned().collect());

        for (data_key, request) in requests {
            let Some(node_id) = resolve_effect_node(app, &request.source) else {
                if let Some(node) = self.compressor_nodes.remove(&data_key) {
                    let _ =
                        unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
                }
                continue;
            };
            let needs_watch = self
                .compressor_nodes
                .get(&data_key)
                .map(|node| node.node_id != node_id)
                .unwrap_or(true);
            if needs_watch {
                if let Some(node) = self.compressor_nodes.remove(&data_key) {
                    let _ =
                        unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
                }
                if !unsafe { audiograph::add_node_to_watchlist(self.lg.0, node_id) } {
                    continue;
                }
                self.compressor_nodes.insert(
                    data_key.clone(),
                    CompressorMeterNode {
                        node_id,
                        revision: 0,
                        last_ring_write: -1.0,
                    },
                );
            }
            if !poll_due {
                continue;
            }
            let Some(node) = self.compressor_nodes.get_mut(&data_key) else {
                continue;
            };
            let mut state = vec![0.0f32; comp::COMPRESSOR_STATE_SIZE];
            let state_bytes = state.len() * std::mem::size_of::<f32>();
            let mut state_size = 0usize;
            let copied = unsafe {
                audiograph::get_node_state_into(
                    self.lg.0,
                    node.node_id,
                    state.as_mut_ptr().cast(),
                    state_bytes,
                    &mut state_size as *mut usize,
                )
            };
            if !copied || state_size < state_bytes {
                continue;
            }
            let ring_write = state[comp::STATE_RING_WRITE];
            if ring_write == node.last_ring_write {
                continue;
            }
            node.last_ring_write = ring_write;
            node.revision += 1;
            // Unroll the ring so history reads oldest..newest.
            let head = (ring_write.max(0.0) as usize) % comp::METER_RING_LEN;
            let mut history = Vec::with_capacity(comp::METER_RING_LEN);
            for i in 0..comp::METER_RING_LEN {
                let entry = (head + i) % comp::METER_RING_LEN;
                history.push([
                    state[comp::STATE_METER_RING + entry * 2],
                    state[comp::STATE_METER_RING + entry * 2 + 1],
                ]);
            }
            eseqlisp::live_audio::publish_compressor_meter_frame(
                data_key,
                CompressorMeterFrame {
                    revision: node.revision,
                    gr_db: state[comp::STATE_METER_GR_DB],
                    out_db: state[comp::STATE_METER_OUT_DB],
                    sample_rate: state[comp::STATE_SAMPLE_RATE],
                    stride: comp::METER_STRIDE,
                    history: Arc::new(history),
                },
            );
            changed = true;
        }
        changed
    }

    pub(crate) fn suspend_for_project_load(&mut self) -> bool {
        let had_live_data = !self.taps.is_empty()
            || !self.processors.is_empty()
            || !self.meter_nodes.is_empty()
            || !self.compressor_nodes.is_empty();
        self.clear_taps();
        self.processors.clear();
        self.clear_meter_nodes();
        eseqlisp::live_audio::retain_spectrogram_frames(&HashSet::<String>::new());
        eseqlisp::live_audio::retain_scope_frames(&HashSet::<String>::new());
        eseqlisp::live_audio::retain_band_meter_frames(&HashSet::<String>::new());
        eseqlisp::live_audio::retain_compressor_meter_frames(&HashSet::<String>::new());
        had_live_data
    }

    fn create_tap(
        &self,
        tap_key: &TapKey,
        source_node: i32,
        requested_ring_frames: usize,
    ) -> Option<TapNode> {
        let ring_frames = audio_tap::normalize_ring_frames(requested_ring_frames);
        let initial_state = audio_tap::initial_state(ring_frames);
        let name = CString::new(tap_key.label()).ok()?;
        let node_id = unsafe {
            let _batch = GraphEditBatchGuard::new(self.lg);
            let node_id = audiograph::add_node(
                self.lg.0,
                audio_tap::audio_tap_vtable(),
                audio_tap::state_size_bytes(ring_frames),
                name.as_ptr(),
                2,
                0,
                initial_state.as_ptr() as *const c_void,
                initial_state.len() * std::mem::size_of::<f32>(),
            );
            if node_id < 0 {
                return None;
            }
            let connected_l = audiograph::graph_connect(self.lg.0, source_node, 0, node_id, 0);
            let connected_r = audiograph::graph_connect(self.lg.0, source_node, 1, node_id, 1);
            if !connected_l || !connected_r {
                let _ = audiograph::graph_disconnect(self.lg.0, source_node, 0, node_id, 0);
                let _ = audiograph::graph_disconnect(self.lg.0, source_node, 1, node_id, 1);
                let _ = audiograph::delete_node(self.lg.0, node_id);
                return None;
            }
            node_id
        };
        let watched = unsafe { audiograph::add_node_to_watchlist(self.lg.0, node_id) };
        if !watched {
            self.destroy_tap(TapNode {
                node_id,
                source_node,
                ring_frames,
                state: Vec::new(),
            });
            return None;
        }

        Some(TapNode {
            node_id,
            source_node,
            ring_frames,
            state: vec![0.0; audio_tap::state_len_floats(ring_frames)],
        })
    }

    fn destroy_tap(&self, tap: TapNode) {
        let _ = unsafe { audiograph::remove_node_from_watchlist(self.lg.0, tap.node_id) };
        unsafe {
            let _batch = GraphEditBatchGuard::new(self.lg);
            let _ = audiograph::graph_disconnect(self.lg.0, tap.source_node, 0, tap.node_id, 0);
            let _ = audiograph::graph_disconnect(self.lg.0, tap.source_node, 1, tap.node_id, 1);
            let _ = audiograph::delete_node(self.lg.0, tap.node_id);
        }
    }

    fn clear_taps(&mut self) {
        let taps = std::mem::take(&mut self.taps);
        for (_, tap) in taps {
            self.destroy_tap(tap);
        }
    }

    fn clear_meter_nodes(&mut self) {
        for (_, node) in std::mem::take(&mut self.meter_nodes) {
            let _ = unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
        }
        for (_, node) in std::mem::take(&mut self.compressor_nodes) {
            let _ = unsafe { audiograph::remove_node_from_watchlist(self.lg.0, node.node_id) };
        }
    }
}

impl Drop for LiveAudioAnalyzerManager {
    fn drop(&mut self) {
        self.clear_taps();
        self.clear_meter_nodes();
    }
}

struct GraphEditBatchGuard {
    lg: LiveGraphPtr,
}

impl GraphEditBatchGuard {
    fn new(lg: LiveGraphPtr) -> Self {
        unsafe { audiograph::begin_graph_edit_batch(lg.0) };
        Self { lg }
    }
}

impl Drop for GraphEditBatchGuard {
    fn drop(&mut self) {
        unsafe { audiograph::end_graph_edit_batch(self.lg.0) };
    }
}

/// Resolves an effect-slot source straight to its node id (for widgets that
/// read the effect node's own state rather than tapping its audio).
fn resolve_effect_node(app: &app::App, source: &LiveAudioSourceSelector) -> Option<i32> {
    match source {
        LiveAudioSourceSelector::TrackEffect { index, slot } => app
            .state
            .pattern
            .effect_chains
            .get(*index)
            .and_then(|chain| chain.get(*slot))
            .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::RackEffect {
            index,
            rack_slot,
            slot,
        } => app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(*index)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(*rack_slot))
            .and_then(|rack_slot| rack_slot.effect_slots.get(*slot))
            .map(|slot| slot.node_id as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::BusEffect {
            id: Some(bus_id),
            slot,
            ..
        } => app
            .buses
            .iter()
            .find(|bus| bus.id == BusId(*bus_id))
            .and_then(|bus| bus.effect_slots.get(*slot))
            .map(|slot| slot.node_id as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::BusEffect {
            id: None,
            index: Some(index),
            slot,
        } => app
            .buses
            .get(*index)
            .and_then(|bus| bus.effect_slots.get(*slot))
            .map(|slot| slot.node_id as i32)
            .filter(|node_id| *node_id > 0),
        _ => None,
    }
}

fn resolve_source_node(app: &app::App, tap_key: &TapKey) -> Option<i32> {
    match &tap_key.source {
        LiveAudioSourceSelector::Master => app
            .graph
            .bus_node_ids
            .iter()
            .find(|bus| bus.id == BusId::MIX)
            .map(|bus| node_for_bus(bus, tap_key.tap_point)),
        LiveAudioSourceSelector::Track { index } => {
            app.graph
                .track_node_ids
                .get(*index)
                .map(|track| match tap_key.tap_point {
                    TapPoint::PreFx => track.pan_id,
                    TapPoint::PostFx => track.delay_id,
                })
        }
        LiveAudioSourceSelector::TrackEffect { index, slot } => app
            .state
            .pattern
            .effect_chains
            .get(*index)
            .and_then(|chain| chain.get(*slot))
            .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::RackEffect {
            index,
            rack_slot,
            slot,
        } => app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(*index)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(*rack_slot))
            .and_then(|rack_slot| rack_slot.effect_slots.get(*slot))
            .map(|slot| slot.node_id as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::Bus {
            id: Some(bus_id), ..
        } => app
            .graph
            .bus_node_ids
            .iter()
            .find(|bus| bus.id == BusId(*bus_id))
            .map(|bus| node_for_bus(bus, tap_key.tap_point)),
        LiveAudioSourceSelector::Bus {
            id: None,
            index: Some(index),
        } => app
            .graph
            .bus_node_ids
            .get(*index)
            .map(|bus| node_for_bus(bus, tap_key.tap_point)),
        LiveAudioSourceSelector::Bus {
            id: None,
            index: None,
        } => None,
        LiveAudioSourceSelector::BusEffect {
            id: Some(bus_id),
            slot,
            ..
        } => app
            .buses
            .iter()
            .find(|bus| bus.id == BusId(*bus_id))
            .and_then(|bus| bus.effect_slots.get(*slot))
            .map(|slot| slot.node_id as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::BusEffect {
            id: None,
            index: Some(index),
            slot,
        } => app
            .buses
            .get(*index)
            .and_then(|bus| bus.effect_slots.get(*slot))
            .map(|slot| slot.node_id as i32)
            .filter(|node_id| *node_id > 0),
        LiveAudioSourceSelector::BusEffect {
            id: None,
            index: None,
            ..
        } => None,
    }
}

fn node_for_bus(bus: &app::BusNodeIds, tap_point: TapPoint) -> i32 {
    match tap_point {
        TapPoint::PreFx => bus.gate_id,
        TapPoint::PostFx => bus.volume_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_load_suspension_clears_analyzer_publish_state() {
        eseqlisp::live_audio::clear_spectrogram_frames();
        let mut manager = LiveAudioAnalyzerManager::new(LiveGraphPtr(std::ptr::null_mut()));
        manager.processors.insert(
            "spectrogram:test".to_string(),
            SpectrogramProcessor::new(2048, 8, -72.0, 0.0, 0.5),
        );
        eseqlisp::live_audio::publish_spectrogram_frame(
            "spectrogram:test",
            eseqlisp::live_audio::SpectrogramFrame {
                revision: 1,
                bins: 2,
                time_slices: 2,
                write_head: 1,
                sample_rate: 48_000.0,
                waterfall: Arc::new(vec![0.0; 4]),
                smoothed: Arc::new(vec![0.0; 2]),
            },
        );

        assert!(manager.suspend_for_project_load());

        assert!(manager.processors.is_empty());
        assert!(eseqlisp::live_audio::spectrogram_frame("spectrogram:test").is_none());
        eseqlisp::live_audio::clear_spectrogram_frames();
    }
}
