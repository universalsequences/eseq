use super::super::*;

impl SequencerState {
    pub fn insert_effect_slot_in_other_track_patterns(&self, track: usize, slot_idx: usize) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.insert_empty_effect_slot(slot_idx));
    }

    pub fn move_effect_slot_in_other_track_patterns(
        &self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.move_effect_slot_to(source_slot, target_slot);
        });
    }

    pub fn remove_effect_slot_from_track_patterns(&self, track: usize, slot_idx: usize) {
        self.save_current_track_effect_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.remove_effect_slot(slot_idx));
    }

    pub fn insert_midi_fx_slot_in_other_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        name: String,
        descriptor: &EffectDescriptor,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.insert_midi_fx_slot(slot_idx, name.clone(), descriptor);
        });
    }

    pub fn replace_midi_fx_slot_in_all_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        name: String,
        descriptor: &EffectDescriptor,
    ) -> Result<(), String> {
        if track >= self.pattern.track_params.len()
            || slot_idx >= self.pattern.midi_fx_slots[track].len()
        {
            return Err("MIDI-FX replacement target is out of range".to_string());
        }
        let mut live_chain = self.pattern.track_params[track].midi_fx_chain();
        if slot_idx >= live_chain.len() {
            return Err("MIDI-FX replacement target is empty".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        // Validate and mutate only entities the model can still reach:
        // orphans (un-pruned leftovers of pattern deletes and relinks) may
        // predate this slot entirely, and letting one veto the edit would
        // block it forever.
        let referenced: HashSet<PatchId> = scenes
            .referenced_track_sounds(track)
            .into_iter()
            .map(|refs| refs.patch)
            .collect();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.sounds.patches.iter().any(|(id, patch)| {
            referenced.contains(id)
                && (slot_idx >= patch.params.midi_fx_chain.len()
                    || slot_idx >= patch.midi_fx_slots.len())
        }) {
            return Err("stored MIDI-FX replacement target is missing".to_string());
        }
        live_chain[slot_idx] = name.clone();
        self.pattern.track_params[track].set_midi_fx_chain(live_chain);
        self.pattern.midi_fx_slots[track][slot_idx].apply_descriptor(descriptor, 0);
        for (id, patch) in pool.sounds.patches.iter_mut().map(|(id, entity)| (id, Arc::make_mut(entity))) {
            if !referenced.contains(id) {
                continue;
            }
            patch.params.midi_fx_chain[slot_idx] = name.clone();
            patch.midi_fx_slots[slot_idx].sync_to_descriptor(descriptor, 0);
        }
        Ok(())
    }

    pub fn move_midi_fx_slot_in_other_track_patterns(
        &self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.move_midi_fx_slot_to(source_slot, target_slot);
        });
    }

    pub fn remove_midi_fx_slot_from_track_patterns(&self, track: usize, slot_idx: usize) {
        self.save_current_track_midi_fx_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.remove_midi_fx_slot(slot_idx));
    }

    pub fn remove_bus_references_from_all_track_patterns(&self, bus_id: BusId) {
        // Whole-pool sweep is fine here (unlike the MIDI-FX replace above):
        // idempotent, cannot fail, and scrubbing a dead bus from an orphan
        // entity is harmless.
        let mut scenes = self.pattern.scenes.lock().unwrap();
        for pool in &mut scenes.track_pools {
            for mix in pool.sounds.mixes.values_mut().map(Arc::make_mut) {
                if mix.output == TrackOutput::Bus(bus_id) {
                    mix.output = TrackOutput::Mix;
                }
                mix.sends.retain(|send| send.destination != bus_id);
            }
        }
    }

    /// Force one track's output across every stored scene. Track output is
    /// otherwise per-scene, but a track group is a global concept — its members
    /// must reach the backing bus in every scene, or switching scenes would tear
    /// the group's routing apart (and a saved project would silently lose it).
    pub fn set_track_output_in_all_track_patterns(
        &self,
        track: usize,
        output: TrackOutput,
    ) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let mut changed = false;
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for mix in pool.sounds.mixes.values_mut().map(Arc::make_mut) {
                if mix.output != output {
                    mix.output = output.clone();
                    changed = true;
                }
            }
        }
        changed
    }
}
