/*!
Custom-engine voice allocation and audio-runtime topology synchronization.

`CustomEnginePool` tracks per-engine voice slots with allocation, LRU-ish
stealing, release tails, and free-patch (always-running) voices. The sync
half reconciles pools against the current track topology each block: sampler
pool sizing, rack pool wiring, free-patch transport routing, and the
full `reset_audio_runtime_for_track_topology` reset, plus voice-release and
mute-group enforcement across tracks and racks.
*/

#[allow(unused_imports)]
use super::{VoicePool, MAX_VOICES};
use crate::audio::*;

pub(in crate::audio) struct CustomVoiceSlot {
    pub(in crate::audio) logical_id: u64,
    pub(in crate::audio) age: u64,
    pub(in crate::audio) active: bool,
    pub(in crate::audio) release_started_sample: Option<u64>,
    pub(in crate::audio) note: f32,
    pub(in crate::audio) assigned_track: Option<usize>,
    pub(in crate::audio) assigned_route: Option<usize>,
    pub(in crate::audio) fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::audio) struct CustomVoiceAllocation {
    pub(in crate::audio) voice_idx: usize,
    pub(in crate::audio) logical_id: u64,
    pub(in crate::audio) previous_track: Option<usize>,
    pub(in crate::audio) previous_route: Option<usize>,
    pub(in crate::audio) stole_active_voice: bool,
}

pub(in crate::audio) struct CustomEnginePool {
    pub(in crate::audio) voices: [CustomVoiceSlot; MAX_VOICES],
    pub(in crate::audio) num_voices: usize,
    pub(in crate::audio) enabled_voice_count: usize,
    pub(in crate::audio) age_counter: u64,
}

impl CustomEnginePool {
    pub(in crate::audio) fn new() -> Self {
        Self {
            voices: std::array::from_fn(|_| CustomVoiceSlot {
                logical_id: 0,
                age: 0,
                active: false,
                release_started_sample: None,
                note: 0.0,
                assigned_track: None,
                assigned_route: None,
                fingerprint: 0,
            }),
            num_voices: 0,
            enabled_voice_count: 1,
            age_counter: 0,
        }
    }

    pub(in crate::audio) fn add_voice(&mut self, logical_id: u64) {
        if self.num_voices < MAX_VOICES {
            self.voices[self.num_voices] = CustomVoiceSlot {
                logical_id,
                age: 0,
                active: false,
                release_started_sample: None,
                note: 0.0,
                assigned_track: None,
                assigned_route: None,
                fingerprint: 0,
            };
            self.num_voices += 1;
        }
    }

    pub(in crate::audio) fn reset(&mut self) {
        self.num_voices = 0;
        self.enabled_voice_count = 1;
        self.age_counter = 0;
        for voice in &mut self.voices {
            *voice = CustomVoiceSlot {
                logical_id: 0,
                age: 0,
                active: false,
                release_started_sample: None,
                note: 0.0,
                assigned_track: None,
                assigned_route: None,
                fingerprint: 0,
            };
        }
    }

    pub(in crate::audio) fn allocate_voice(
        &mut self,
        track: usize,
        route_idx: usize,
        note: f32,
        polyphonic: bool,
        max_polyphony: usize,
    ) -> CustomVoiceAllocation {
        self.age_counter += 1;
        let max_polyphony = max_polyphony.clamp(1, MAX_VOICES);
        if !polyphonic {
            if let Some(idx) =
                (0..self.num_voices).find(|&i| self.voices[i].assigned_route == Some(route_idx))
            {
                let slot = &mut self.voices[idx];
                let previous_track = slot.assigned_track;
                let previous_route = slot.assigned_route;
                let stole_active_voice = slot.active;
                slot.age = self.age_counter;
                slot.active = true;
                slot.release_started_sample = None;
                slot.note = note;
                slot.assigned_track = Some(track);
                slot.assigned_route = Some(route_idx);
                return CustomVoiceAllocation {
                    voice_idx: idx,
                    logical_id: slot.logical_id,
                    previous_track,
                    previous_route,
                    stole_active_voice,
                };
            }
        }

        let mut active_same_note_idx = None;
        let mut releasing_same_note_idx = None;
        let mut releasing_same_note_age = u64::MAX;
        let mut idle_same_track_idx = None;
        let mut idle_same_track_age = u64::MAX;
        let mut releasing_same_track_idx = None;
        let mut releasing_same_track_age = u64::MAX;
        let mut unassigned_idle_idx = None;
        let mut unassigned_idle_age = u64::MAX;
        let mut oldest_same_track = None;
        let mut oldest_same_track_age = u64::MAX;
        let mut assigned_same_track_count = 0usize;
        let mut idle_other_track_idx = None;
        let mut idle_other_track_age = u64::MAX;
        let mut releasing_other_track_idx = None;
        let mut releasing_other_track_age = u64::MAX;
        let mut oldest_idx = 0;
        let mut oldest_age = u64::MAX;

        for i in 0..self.num_voices {
            let voice = &self.voices[i];
            if !voice.active {
                let is_releasing = voice.release_started_sample.is_some();
                match voice.assigned_track {
                    Some(_) if voice.assigned_route == Some(route_idx) => {
                        if is_releasing {
                            if (voice.note - note).abs() < 0.01
                                && voice.age < releasing_same_note_age
                            {
                                releasing_same_note_idx = Some(i);
                                releasing_same_note_age = voice.age;
                            }
                            if voice.age < releasing_same_track_age {
                                releasing_same_track_idx = Some(i);
                                releasing_same_track_age = voice.age;
                            }
                        } else if voice.age < idle_same_track_age {
                            idle_same_track_idx = Some(i);
                            idle_same_track_age = voice.age;
                        }
                    }
                    Some(_) => {
                        if is_releasing {
                            if voice.age < releasing_other_track_age {
                                releasing_other_track_idx = Some(i);
                                releasing_other_track_age = voice.age;
                            }
                        } else if voice.age < idle_other_track_age {
                            idle_other_track_idx = Some(i);
                            idle_other_track_age = voice.age;
                        }
                    }
                    None => {
                        if !is_releasing && voice.age < unassigned_idle_age {
                            unassigned_idle_idx = Some(i);
                            unassigned_idle_age = voice.age;
                        }
                    }
                }
            }
            if voice.active
                && voice.assigned_route == Some(route_idx)
                && (voice.note - note).abs() < 0.01
            {
                active_same_note_idx = Some(i);
            }
            if voice.assigned_route == Some(route_idx) {
                assigned_same_track_count += 1;
                if voice.age < oldest_same_track_age {
                    oldest_same_track = Some(i);
                    oldest_same_track_age = voice.age;
                }
            }
            if voice.age < oldest_age {
                oldest_idx = i;
                oldest_age = voice.age;
            }
        }

        let idx = if assigned_same_track_count >= max_polyphony {
            active_same_note_idx
                .or(releasing_same_note_idx)
                .or(idle_same_track_idx)
                .or(releasing_same_track_idx)
                .or(oldest_same_track)
                .unwrap_or(oldest_idx)
        } else {
            active_same_note_idx
                .or(releasing_same_note_idx)
                .or(idle_same_track_idx)
                .or(unassigned_idle_idx)
                .or(idle_other_track_idx)
                .or(releasing_same_track_idx)
                .or(oldest_same_track)
                .or(releasing_other_track_idx)
                .unwrap_or(oldest_idx)
        };
        let slot = &mut self.voices[idx];
        let previous_track = slot.assigned_track;
        let previous_route = slot.assigned_route;
        let stole_active_voice = slot.active;
        slot.age = self.age_counter;
        slot.active = true;
        slot.release_started_sample = None;
        slot.note = note;
        slot.assigned_track = Some(track);
        slot.assigned_route = Some(route_idx);
        CustomVoiceAllocation {
            voice_idx: idx,
            logical_id: slot.logical_id,
            previous_track,
            previous_route,
            stole_active_voice,
        }
    }

    pub(in crate::audio) fn allocate_free_patch_voice(
        &mut self,
        track: usize,
        route_idx: usize,
        note: f32,
    ) -> Option<CustomVoiceAllocation> {
        if self.num_voices == 0 {
            return None;
        }
        self.age_counter += 1;
        let slot = &mut self.voices[0];
        let previous_track = slot.assigned_track;
        let previous_route = slot.assigned_route;
        let stole_active_voice = slot.active;
        slot.age = self.age_counter;
        slot.active = true;
        slot.release_started_sample = None;
        slot.note = note;
        slot.assigned_track = Some(track);
        slot.assigned_route = Some(route_idx);
        Some(CustomVoiceAllocation {
            voice_idx: 0,
            logical_id: slot.logical_id,
            previous_track,
            previous_route,
            stole_active_voice,
        })
    }

    pub(in crate::audio) fn release_voice_by_logical_id(&mut self, logical_id: u64, release_sample: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                self.voices[i].release_started_sample = Some(release_sample);
                return;
            }
        }
    }

    pub(in crate::audio) fn release_free_patch_voice_by_logical_id(&mut self, logical_id: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                self.voices[i].release_started_sample = None;
                return;
            }
        }
    }

    pub(in crate::audio) fn note_voice_allocated(&mut self, engine_id: usize, voice_idx: usize) {
        let needed = (voice_idx + 1).min(MAX_VOICES).max(1);
        if needed > self.enabled_voice_count {
            self.enabled_voice_count = needed;
            crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, needed);
        }
    }

    pub(in crate::audio) fn sync_enabled_voice_count(&mut self, engine_id: usize) {
        self.enabled_voice_count = crate::lisp_host::get_dgen_engine_enabled_voices(engine_id);
    }

    pub(in crate::audio) fn shrink_released_voices(
        &mut self,
        engine_id: usize,
        current_sample: u64,
        release_tail_samples: u64,
        minimum_enabled_voices: usize,
    ) {
        let mut highest_retained_idx: Option<usize> = None;
        for i in 0..self.num_voices {
            let voice = &mut self.voices[i];
            if let Some(release_started_sample) = voice.release_started_sample {
                if current_sample.saturating_sub(release_started_sample) >= release_tail_samples {
                    voice.release_started_sample = None;
                }
            }
            if voice.active || voice.release_started_sample.is_some() {
                highest_retained_idx =
                    Some(highest_retained_idx.map_or(i, |highest| highest.max(i)));
            }
        }

        let needed = highest_retained_idx
            .map(|highest| highest + 1)
            .unwrap_or(minimum_enabled_voices)
            .clamp(minimum_enabled_voices, MAX_VOICES);
        if needed < self.enabled_voice_count {
            self.enabled_voice_count = needed;
            crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, needed);
        }
    }

    pub(in crate::audio) fn invalidate_sound_cache(&mut self) {
        for i in 0..self.num_voices {
            self.voices[i].fingerprint = 0;
        }
    }
}

pub(in crate::audio) fn custom_engine_requires_idle_voice(
    data: &AudioCallbackData,
    engine_id: usize,
    num_tracks: usize,
) -> bool {
    let num_tracks = num_tracks.min(data.scheduler_snapshot.tracks.len());
    (0..num_tracks).any(|track_idx| {
        if track_engine_id(&data.state, track_idx) == Some(engine_id)
            && track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch
        {
            return true;
        }
        data.scheduler_snapshot.tracks[track_idx]
            .rack_track
            .as_ref()
            .is_some_and(|rack| {
                rack.slots.iter().any(|slot| {
                    slot.track_sound_state.engine_id == Some(engine_id)
                        && slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch
                })
            })
    })
}

pub(in crate::audio) fn sync_sampler_voice_pool(state: &SequencerState, track: usize, pool: &mut VoicePool) {
    let desired_count = state.runtime.voice_counts[track].load(Ordering::Acquire) as usize;
    let desired_count = desired_count.min(MAX_VOICES);

    let mut needs_reset = pool.num_voices != desired_count;
    if !needs_reset {
        for v in 0..desired_count {
            let desired_lid = state.runtime.voice_lids[track][v].load(Ordering::Acquire);
            let desired_node_id = state.runtime.synth_node_ids[track][v].load(Ordering::Acquire);
            let desired_gatepitch_id =
                state.runtime.sampler_gatepitch_node_ids[track][v].load(Ordering::Acquire);
            let desired_modulator_id =
                state.runtime.sampler_modulator_node_ids[track][v].load(Ordering::Acquire);
            if pool.voices[v].logical_id != desired_lid
                || pool.voices[v].node_id as u32 != desired_node_id
                || pool.voices[v].gatepitch_id as u32 != desired_gatepitch_id
                || pool.voices[v].modulator_id as u32 != desired_modulator_id
            {
                needs_reset = true;
                break;
            }
        }
    }

    if needs_reset {
        pool.reset();
        for v in 0..desired_count {
            let lid = state.runtime.voice_lids[track][v].load(Ordering::Acquire);
            if lid != 0 {
                let node_id = state.runtime.synth_node_ids[track][v].load(Ordering::Acquire) as i32;
                let gatepitch_id = state.runtime.sampler_gatepitch_node_ids[track][v]
                    .load(Ordering::Acquire) as i32;
                let modulator_id = state.runtime.sampler_modulator_node_ids[track][v]
                    .load(Ordering::Acquire) as i32;
                pool.add_modulated_voice(lid, node_id, gatepitch_id, modulator_id);
            }
        }
    }
}

pub(in crate::audio) fn sync_custom_engine_pool(state: &SequencerState, engine_id: usize, pool: &mut CustomEnginePool) {
    let desired_count =
        state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
    let desired_count = desired_count.min(MAX_VOICES);

    let mut needs_reset = pool.num_voices != desired_count;
    if !needs_reset {
        for v in 0..desired_count {
            let desired_lid = state.runtime.engine_voice_lids[engine_id][v].load(Ordering::Acquire);
            if pool.voices[v].logical_id != desired_lid {
                needs_reset = true;
                break;
            }
        }
    }

    if needs_reset {
        pool.reset();
        crate::lisp_host::reset_dgen_engine_enabled_voices(engine_id);
        for v in 0..desired_count {
            let lid = state.runtime.engine_voice_lids[engine_id][v].load(Ordering::Acquire);
            if lid != 0 {
                pool.add_voice(lid);
            }
        }
    } else {
        pool.sync_enabled_voice_count(engine_id);
    }
}

pub(in crate::audio) fn sync_rack_voice_pools(data: &mut AudioCallbackData, num_tracks: usize) {
    // Iterate the snapshot by reference instead of cloning each RackTrackSnapshot
    // (and its nested EffectSlotSnapshot/Vec fields). This runs every audio
    // callback, so a deep clone here was a real-time-thread heap-allocation
    // storm unrelated to voice count or polyphony.
    let num_tracks = num_tracks.min(data.scheduler_snapshot.tracks.len());
    for track_idx in 0..num_tracks {
        let Some(rack) = data.scheduler_snapshot.tracks[track_idx]
            .rack_track
            .as_ref()
        else {
            continue;
        };
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            match slot.instrument_type {
                InstrumentType::Sampler => {
                    let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                        continue;
                    };
                    if pool_id < data.voice_pools.len() {
                        sync_sampler_voice_pool(
                            &data.state,
                            pool_id,
                            &mut data.voice_pools[pool_id],
                        );
                    }
                }
                InstrumentType::Custom => {
                    let Some(engine_id) = slot.track_sound_state.engine_id else {
                        continue;
                    };
                    if engine_id < data.custom_engine_pools.len() {
                        sync_custom_engine_pool(
                            &data.state,
                            engine_id,
                            &mut data.custom_engine_pools[engine_id],
                        );
                    }
                }
                InstrumentType::Modulator | InstrumentType::Rack => {}
            }
        }
    }
}

pub(in crate::audio) fn free_patch_route_lids_hash(
    state: &SequencerState,
    engine_id: usize,
    num_tracks: usize,
) -> Option<u64> {
    if engine_id >= state.runtime.engine_route_lids.len() {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    num_tracks.hash(&mut hasher);
    for track in 0..num_tracks.min(MAX_TRACKS) {
        state.runtime.engine_route_lids[engine_id][0][track]
            .load(Ordering::Acquire)
            .hash(&mut hasher);
        state.runtime.engine_route_lids_r[engine_id][0][track]
            .load(Ordering::Acquire)
            .hash(&mut hasher);
        for input in 0..crate::sequencer::EXT_MOD_INPUT_COUNT {
            state.runtime.engine_ext_route_lids[engine_id][0][track][input]
                .load(Ordering::Acquire)
                .hash(&mut hasher);
        }
    }
    Some(hasher.finish())
}

pub(in crate::audio) fn free_patch_transport_route_target(
    state: &SequencerState,
    track: usize,
    num_tracks: usize,
    playing: bool,
) -> Option<FreePatchTransportRouteTarget> {
    if track >= num_tracks || track >= MAX_TRACKS {
        return None;
    }
    if InstrumentType::from_runtime_flag(
        state.runtime.instrument_type_flags[track].load(Ordering::Acquire),
    ) != InstrumentType::Custom
    {
        return None;
    }
    if track_custom_run_mode(state, track) != CustomInstrumentRunMode::FreePatch {
        return None;
    }
    let engine_id = track_engine_id(state, track)?;
    let route_hash = free_patch_route_lids_hash(state, engine_id, num_tracks)?;
    Some(FreePatchTransportRouteTarget {
        engine_id,
        route_hash,
        open: playing,
    })
}

pub(in crate::audio) fn free_patch_transport_route_cache_is_fresh(
    cached: FreePatchTransportRouteState,
    target: FreePatchTransportRouteTarget,
) -> bool {
    cached.valid
        && cached.engine_id == target.engine_id
        && cached.route_hash == target.route_hash
        && cached.open == target.open
        && target.open
}

pub(in crate::audio) unsafe fn set_free_patch_transport_route(
    lg: *mut LiveGraph,
    state: &SequencerState,
    engine_id: usize,
    track: usize,
    num_tracks: usize,
    open: bool,
) {
    if engine_id >= state.runtime.engine_route_lids.len() {
        return;
    }

    for route_track in 0..num_tracks.min(MAX_TRACKS) {
        let value = if open && route_track == track {
            1.0
        } else {
            0.0
        };
        let lid_l =
            state.runtime.engine_route_lids[engine_id][0][route_track].load(Ordering::Acquire);
        if lid_l != 0 {
            params_push_wrapper(
                lg,
                ParamMsg {
                    idx: 0,
                    logical_id: lid_l,
                    fvalue: value,
                },
            );
        }

        let lid_r =
            state.runtime.engine_route_lids_r[engine_id][0][route_track].load(Ordering::Acquire);
        if lid_r != 0 {
            params_push_wrapper(
                lg,
                ParamMsg {
                    idx: 0,
                    logical_id: lid_r,
                    fvalue: value,
                },
            );
        }

        for input in 0..crate::sequencer::EXT_MOD_INPUT_COUNT {
            let ext_lid = state.runtime.engine_ext_route_lids[engine_id][0][route_track][input]
                .load(Ordering::Acquire);
            if ext_lid != 0 {
                params_push_wrapper(
                    lg,
                    ParamMsg {
                        idx: 0,
                        logical_id: ext_lid,
                        fvalue: value,
                    },
                );
            }
        }
    }
}

pub(in crate::audio) fn sync_free_patch_transport_routes(data: &mut AudioCallbackData, num_tracks: usize) {
    let playing = data.state.transport.playing.load(Ordering::Acquire);
    for track in 0..MAX_TRACKS {
        let Some(target) =
            free_patch_transport_route_target(&data.state, track, num_tracks, playing)
        else {
            data.free_patch_transport_routes[track].valid = false;
            continue;
        };

        let cached = data.free_patch_transport_routes[track];
        if free_patch_transport_route_cache_is_fresh(cached, target) {
            continue;
        }

        unsafe {
            set_free_patch_transport_route(
                data.lg.0,
                &data.state,
                target.engine_id,
                track,
                num_tracks,
                target.open,
            );
        }
        data.free_patch_transport_routes[track] = FreePatchTransportRouteState {
            valid: true,
            engine_id: target.engine_id,
            route_hash: target.route_hash,
            open: target.open,
        };
    }
}

/// Reconcile an event-compatible topology publication without disturbing
/// voices or queued events. Track appends initialize only new pools; rack slot
/// changes resync their route-backed pools while every existing track keeps
/// its callback-local voice and transport state.
pub(in crate::audio) fn reconcile_event_compatible_topology(
    data: &mut AudioCallbackData,
    num_tracks: usize,
    topology_epoch: u64,
) {
    let first_new_track = data.last_num_tracks.min(num_tracks);
    for track in first_new_track..num_tracks {
        sync_sampler_voice_pool(&data.state, track, &mut data.voice_pools[track]);
        if let Some(engine_id) = track_engine_id(&data.state, track) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
        data.pending_accum_reset[track] = true;
    }
    sync_rack_voice_pools(data, num_tracks);
    data.last_num_tracks = num_tracks;
    data.last_topology_epoch = topology_epoch;
    data.pending_topology_delete_track = None;
}

pub(in crate::audio) fn remap_route_after_track_delete(
    route: usize,
    deleted_track: usize,
) -> Option<usize> {
    if route < MAX_TRACKS {
        return match route.cmp(&deleted_track) {
            std::cmp::Ordering::Less => Some(route),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(route - 1),
        };
    }
    let offset = route - MAX_TRACKS;
    let route_track = offset / MAX_RACK_SLOTS;
    let slot = offset % MAX_RACK_SLOTS;
    match route_track.cmp(&deleted_track) {
        std::cmp::Ordering::Less => Some(route),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => rack_slot_pool_index(route_track - 1, slot),
    }
}

fn remap_track_index_after_delete(track: &mut usize, deleted_track: usize) -> bool {
    match (*track).cmp(&deleted_track) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Greater => {
            *track -= 1;
            true
        }
    }
}

fn remap_scheduled_event_after_track_delete(
    event: &mut ScheduledEvent,
    deleted_track: usize,
    pattern_epoch: u64,
) -> bool {
    let keep = match &mut event.kind {
        ScheduledEventKind::ResolvedTrigger { track, .. }
        | ScheduledEventKind::InstrumentParams { track, .. }
        | ScheduledEventKind::EffectParams { track, .. } => {
            remap_track_index_after_delete(track, deleted_track)
        }
        ScheduledEventKind::NetworkTrigger { track, seed, .. } => {
            if let Some((seed_track, _)) = seed {
                if !remap_track_index_after_delete(seed_track, deleted_track) {
                    *seed = None;
                }
            }
            remap_track_index_after_delete(track, deleted_track)
        }
    };
    if keep {
        event.pattern_epoch = pattern_epoch;
    }
    keep
}

fn reconcile_audio_runtime_after_track_delete(
    data: &mut AudioCallbackData,
    deleted_track: usize,
    num_tracks: usize,
    topology_epoch: u64,
) {
    // The scheduler drained its old-index horizon before authorizing deletion.
    // Callback-local swing delays and gate-offs can legitimately extend past
    // that frontier, so reindex survivors rather than throwing them away.
    let pattern_epoch = data.scheduler_snapshot.transport.pattern_epoch;
    data.countdown_events.retain_mut(|event| {
        let keep = match &mut event.kind {
            CountdownEventKind::Scheduled(scheduled) => {
                remap_scheduled_event_after_track_delete(
                    scheduled,
                    deleted_track,
                    pattern_epoch,
                )
            }
            CountdownEventKind::GateOff(gate_off) => {
                remap_track_index_after_delete(&mut gate_off.track_idx, deleted_track)
            }
            CountdownEventKind::Chop(chop) => {
                remap_track_index_after_delete(&mut chop.track_idx, deleted_track)
            }
        };
        if keep {
            event.pattern_epoch = pattern_epoch;
        }
        keep
    });
    data.block_events.clear();

    for pool in &mut data.custom_engine_pools {
        for voice in &mut pool.voices[..pool.num_voices] {
            match voice.assigned_track.map(|track| track.cmp(&deleted_track)) {
                Some(std::cmp::Ordering::Equal) => {
                    if voice.active {
                        let seq = next_event_sequence_from(&mut data.event_seq);
                        unsafe { send_custom_note_off(data.lg.0, voice.logical_id, 0, seq) };
                    }
                    voice.active = false;
                    voice.release_started_sample = None;
                    voice.assigned_track = None;
                    voice.assigned_route = None;
                }
                Some(std::cmp::Ordering::Greater) => {
                    voice.assigned_track = voice.assigned_track.map(|track| track - 1);
                    voice.assigned_route = voice
                        .assigned_route
                        .and_then(|route| remap_route_after_track_delete(route, deleted_track));
                }
                _ => {}
            }
        }
    }

    for track in deleted_track..num_tracks {
        data.voice_pools.swap(track, track + 1);
        data.active_keyboard_notes.swap(track, track + 1);
        data.rack_choke_last_trigger.swap(track, track + 1);
        for slot in 0..MAX_RACK_SLOTS {
            let current = rack_slot_pool_index(track, slot).expect("validated rack pool");
            let next = rack_slot_pool_index(track + 1, slot).expect("validated rack pool");
            data.voice_pools.swap(current, next);
        }
    }
    data.voice_pools[num_tracks].reset();
    data.active_keyboard_notes[num_tracks] = [None; MAX_VOICES];
    data.rack_choke_last_trigger[num_tracks] = u64::MAX;
    for track in 0..num_tracks {
        for note in data.active_keyboard_notes[track].iter_mut().flatten() {
            for voice in &mut note.voices[..note.voice_count as usize] {
                if let ActiveKeyboardVoiceTarget::Sampler { pool_id } = &mut voice.target {
                    if let Some(remapped) = remap_route_after_track_delete(*pool_id, deleted_track) {
                        *pool_id = remapped;
                    }
                }
            }
        }
    }
    for slot in 0..MAX_RACK_SLOTS {
        let retired = rack_slot_pool_index(num_tracks, slot).expect("validated rack pool");
        data.voice_pools[retired].reset();
    }

    for track in 0..num_tracks {
        sync_sampler_voice_pool(&data.state, track, &mut data.voice_pools[track]);
        if let Some(engine_id) = track_engine_id(&data.state, track) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
    }
    sync_rack_voice_pools(data, num_tracks);
    data.pending_accum_reset = [true; MAX_TRACKS];
    data.last_num_tracks = num_tracks;
    data.last_topology_epoch = topology_epoch;
    data.free_patch_transport_routes = [FreePatchTransportRouteState::default(); MAX_TRACKS];
    data.last_pattern = u32::MAX;
}

pub(in crate::audio) fn reset_audio_runtime_for_track_topology(
    data: &mut AudioCallbackData,
    num_tracks: usize,
    topology_epoch: u64,
    deleted_track: Option<usize>,
) {
    if let Some(deleted_track) = deleted_track.filter(|track| *track <= num_tracks) {
        reconcile_audio_runtime_after_track_delete(
            data,
            deleted_track,
            num_tracks,
            topology_epoch,
        );
        return;
    }
    // Topology edits can invalidate the per-track gate-off bookkeeping for
    // already-ringing custom voices. Explicitly send gate-off to every live
    // custom engine voice before resetting callback-local state so notes do
    // not hang after tracks are compacted.
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        let voice_count =
            data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let lid =
                data.state.runtime.engine_voice_lids[engine_id][voice_idx].load(Ordering::Acquire);
            if lid != 0 {
                let seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_note_off(data.lg.0, lid, 0, seq);
                }
            }
        }
    }

    for pool in &mut data.voice_pools {
        pool.reset();
    }
    for (engine_id, pool) in data.custom_engine_pools.iter_mut().enumerate() {
        pool.reset();
        crate::lisp_host::reset_dgen_engine_enabled_voices(engine_id);
    }
    data.active_keyboard_notes.fill([None; MAX_VOICES]);
    data.pending_accum_reset = [true; MAX_TRACKS];
    data.scheduled_events.clear();
    clear_countdown_events(data);
    data.event_seq = 0;
    data.last_num_tracks = num_tracks;
    data.last_topology_epoch = topology_epoch;
    data.pending_topology_delete_track = None;
    data.last_playing = false;
    // Topology teardown deliberately does not touch host_transport_clock.
    // Clock phase belongs to the continuously running transport; resetting its
    // anchor here leaves every clock-driven existing instrument out of phase
    // until the user restarts playback.
    data.free_patch_transport_routes = [FreePatchTransportRouteState::default(); MAX_TRACKS];
    data.last_pattern = u32::MAX;

    for t in 0..num_tracks {
        sync_sampler_voice_pool(&data.state, t, &mut data.voice_pools[t]);
        if let Some(engine_id) = track_engine_id(&data.state, t) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
    }
    sync_rack_voice_pools(data, num_tracks);
}

pub(in crate::audio) fn publish_active_voice_counts(data: &AudioCallbackData, num_tracks: usize) {
    for track in 0..MAX_TRACKS {
        let active = if track < num_tracks {
            match InstrumentType::from_runtime_flag(
                data.state.runtime.instrument_type_flags[track].load(Ordering::Relaxed),
            ) {
                InstrumentType::Custom => track_engine_id(&data.state, track)
                    .map(|engine_id| {
                        let pool = &data.custom_engine_pools[engine_id];
                        pool.voices[..pool.num_voices]
                            .iter()
                            .filter(|voice| voice.active && voice.assigned_track == Some(track))
                            .count()
                    })
                    .unwrap_or(0),
                InstrumentType::Rack => data
                    .scheduler_snapshot
                    .tracks
                    .get(track)
                    .and_then(|track| track.rack_track.as_ref())
                    .map(|rack| {
                        rack.slots
                            .iter()
                            .enumerate()
                            .map(|(slot_idx, slot)| match slot.instrument_type {
                                InstrumentType::Sampler => rack_slot_pool_index(track, slot_idx)
                                    .and_then(|pool_id| data.voice_pools.get(pool_id))
                                    .map(|pool| {
                                        pool.voices[..pool.num_voices]
                                            .iter()
                                            .filter(|voice| voice.active)
                                            .count()
                                    })
                                    .unwrap_or(0),
                                InstrumentType::Custom => slot
                                    .track_sound_state
                                    .engine_id
                                    .and_then(|engine_id| data.custom_engine_pools.get(engine_id))
                                    .map(|pool| {
                                        pool.voices[..pool.num_voices]
                                            .iter()
                                            .filter(|voice| {
                                                voice.active && voice.assigned_track == Some(track)
                                            })
                                            .count()
                                    })
                                    .unwrap_or(0),
                                InstrumentType::Modulator | InstrumentType::Rack => 0,
                            })
                            .sum()
                    })
                    .unwrap_or(0),
                InstrumentType::Sampler | InstrumentType::Modulator => {
                    let pool = &data.voice_pools[track];
                    pool.voices[..pool.num_voices]
                        .iter()
                        .filter(|voice| voice.active)
                        .count()
                }
            }
        } else {
            0
        };
        data.state.transport.active_voice_counts[track].store(active as u32, Ordering::Relaxed);
    }
}

pub(in crate::audio) fn release_rack_slot_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    release_sample: u64,
    frame_offset: u32,
) {
    let note_offs = collect_rack_slot_active_voice_releases(
        &mut data.voice_pools,
        &mut data.custom_engine_pools,
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
        slot_idx,
        slot,
        release_sample,
    );
    dispatch_rack_slot_note_offs(data, frame_offset, note_offs);
}

pub(in crate::audio) fn release_rack_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    let Some(rack) = data
        .scheduler_snapshot
        .tracks
        .get(track_idx)
        .and_then(|track| track.rack_track.clone())
    else {
        return;
    };
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        release_rack_slot_active_voices(
            data,
            track_idx,
            slot_idx,
            slot,
            release_sample,
            frame_offset,
        );
    }
}

pub(in crate::audio) fn release_track_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    if track_idx >= MAX_TRACKS || track_idx >= data.state.active_track_count() {
        return;
    }
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    data.active_keyboard_notes[track_idx] = [None; MAX_VOICES];

    let instrument_type = InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    );
    if instrument_type == InstrumentType::Modulator {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid != 0 {
            unsafe {
                set_modulator_gate(data.lg.0, lid, 0.0);
            }
        }
        return;
    }
    if instrument_type == InstrumentType::Rack {
        release_rack_active_voices(data, track_idx, release_sample, frame_offset);
        return;
    }

    if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
        let free_patch =
            track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;
        let lids: Vec<u64> = data.custom_engine_pools[engine_id].voices
            [..data.custom_engine_pools[engine_id].num_voices]
            .iter()
            .filter(|voice| voice.active && voice.assigned_track == Some(track_idx))
            .map(|voice| voice.logical_id)
            .collect();
        for lid in lids {
            if free_patch {
                data.custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
            } else {
                data.custom_engine_pools[engine_id]
                    .release_voice_by_logical_id(lid, release_sample);
            }
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, lid, frame_offset, seq);
            }
        }
        return;
    }

    let active: Vec<(u64, i32)> = data.voice_pools[track_idx].voices
        [..data.voice_pools[track_idx].num_voices]
        .iter()
        .filter(|voice| voice.active && voice.logical_id != 0)
        .map(|voice| (voice.logical_id, voice.gatepitch_id))
        .collect();
    for (lid, gatepitch_id) in active {
        data.voice_pools[track_idx].release_voice_by_logical_id(lid);
        cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
        let gatepitch_seq = next_block_event_sequence(data);
        let sampler_seq = next_block_event_sequence(data);
        unsafe {
            if gatepitch_id > 0 {
                send_custom_note_off(data.lg.0, gatepitch_id as u64, frame_offset, gatepitch_seq);
            }
            send_sampler_note_off(data.lg.0, lid, frame_offset, sampler_seq);
        }
    }
}

pub(in crate::audio) fn enforce_mute_group_for_winning_track(
    data: &mut AudioCallbackData,
    winning_track: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    if winning_track >= data.state.active_track_count() {
        return;
    }
    let group = data.state.pattern.track_params[winning_track].get_mute_group();
    if group == 0 {
        return;
    }
    let num_tracks = data.state.active_track_count().min(MAX_TRACKS);
    for track_idx in 0..num_tracks {
        if track_idx == winning_track {
            continue;
        }
        if data.state.pattern.track_params[track_idx].get_mute_group() == group {
            release_track_active_voices(data, track_idx, release_sample, frame_offset);
        }
    }
}
