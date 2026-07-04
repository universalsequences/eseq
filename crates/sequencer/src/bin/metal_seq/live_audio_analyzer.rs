use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CString};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use eseqlisp::widget_render::live_audio::{LiveAudioSourceSelector, TapPoint};
use eseqlisp::widget_render::spectrogram::{collect_spectrogram_requests, SpectrogramRequest};
use sequencer::audio_tap::{self, SpectrogramProcessor};
use sequencer::audiograph::{self, LiveGraphPtr};
use sequencer::sequencer::BusId;
use sequencer::ui;

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

pub(crate) struct LiveAudioAnalyzerManager {
    lg: LiveGraphPtr,
    taps: HashMap<TapKey, TapNode>,
    processors: HashMap<String, SpectrogramProcessor>,
    last_poll_at: Instant,
}

impl LiveAudioAnalyzerManager {
    pub(crate) fn new(lg: LiveGraphPtr) -> Self {
        Self {
            lg,
            taps: HashMap::new(),
            processors: HashMap::new(),
            last_poll_at: Instant::now() - LIVE_AUDIO_ANALYZER_POLL_INTERVAL,
        }
    }

    pub(crate) fn sync_visible(&mut self, editor: &eseqlisp::Editor, app: &ui::App) -> bool {
        if app.has_pending_project_load() {
            return self.suspend_for_project_load();
        }

        let mut grouped: HashMap<TapKey, Vec<SpectrogramRequest>> = HashMap::new();
        for layout in editor.visible_widget_layouts() {
            for request in collect_spectrogram_requests(layout.as_ref()) {
                grouped
                    .entry(TapKey::from_request(&request))
                    .or_default()
                    .push(request);
            }
        }

        let mut active_keys = HashSet::new();
        let mut topology_changed = false;
        let mut active_tap_keys = HashSet::new();
        for (tap_key, requests) in &grouped {
            let Some(source_node) = resolve_source_node(app, tap_key) else {
                continue;
            };
            active_tap_keys.insert(tap_key.clone());
            for request in requests {
                active_keys.insert(request.data_key.clone());
            }
            let required_ring_frames = requests
                .iter()
                .map(|request| request.required_ring_frames)
                .max()
                .map(audio_tap::normalize_ring_frames)
                .unwrap_or(audio_tap::MIN_TAP_RING_FRAMES);

            let needs_recreate = self
                .taps
                .get(tap_key)
                .map(|tap| tap.source_node != source_node || tap.ring_frames < required_ring_frames)
                .unwrap_or(true);
            if needs_recreate {
                if let Some(old_tap) = self.taps.remove(tap_key) {
                    self.destroy_tap(old_tap);
                }
                match self.create_tap(tap_key, source_node, required_ring_frames) {
                    Some(tap) => {
                        self.taps.insert(tap_key.clone(), tap);
                        topology_changed = true;
                    }
                    None => {
                        for request in requests {
                            active_keys.remove(&request.data_key);
                        }
                        active_tap_keys.remove(tap_key);
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

        let mut published_frame = false;
        if self.last_poll_at.elapsed() >= LIVE_AUDIO_ANALYZER_POLL_INTERVAL {
            for (tap_key, requests) in grouped {
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
                for request in requests {
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
            }
            self.last_poll_at = Instant::now();
        }

        topology_changed || published_frame
    }

    pub(crate) fn suspend_for_project_load(&mut self) -> bool {
        let had_live_data = !self.taps.is_empty() || !self.processors.is_empty();
        self.clear_taps();
        self.processors.clear();
        eseqlisp::live_audio::retain_spectrogram_frames(&HashSet::<String>::new());
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
}

impl Drop for LiveAudioAnalyzerManager {
    fn drop(&mut self) {
        self.clear_taps();
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

fn resolve_source_node(app: &ui::App, tap_key: &TapKey) -> Option<i32> {
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

fn node_for_bus(bus: &ui::BusNodeIds, tap_point: TapPoint) -> i32 {
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
