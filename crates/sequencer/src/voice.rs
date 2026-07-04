pub const MAX_VOICES: usize = 12;

pub struct VoiceSlot {
    pub logical_id: u64,
    pub node_id: i32,
    pub gatepitch_id: i32,
    pub modulator_id: i32,
    pub age: u64,
    pub active: bool,
    pub note: f32,
}

pub struct VoicePool {
    pub voices: [VoiceSlot; MAX_VOICES],
    pub num_voices: usize,
    pub polyphonic: bool,
    age_counter: u64,
}

impl VoicePool {
    pub fn new() -> Self {
        Self {
            voices: std::array::from_fn(|_| VoiceSlot {
                logical_id: 0,
                node_id: 0,
                gatepitch_id: 0,
                modulator_id: 0,
                age: 0,
                active: false,
                note: 0.0,
            }),
            num_voices: 0,
            polyphonic: false,
            age_counter: 0,
        }
    }

    pub fn add_voice(&mut self, logical_id: u64, node_id: i32) {
        self.add_modulated_voice(logical_id, node_id, 0, 0);
    }

    pub fn add_modulated_voice(
        &mut self,
        logical_id: u64,
        node_id: i32,
        gatepitch_id: i32,
        modulator_id: i32,
    ) {
        if self.num_voices < MAX_VOICES {
            self.voices[self.num_voices] = VoiceSlot {
                logical_id,
                node_id,
                gatepitch_id,
                modulator_id,
                age: 0,
                active: false,
                note: 0.0,
            };
            self.num_voices += 1;
        }
    }

    pub fn reset(&mut self) {
        self.num_voices = 0;
        self.age_counter = 0;
        for voice in &mut self.voices {
            *voice = VoiceSlot {
                logical_id: 0,
                node_id: 0,
                gatepitch_id: 0,
                modulator_id: 0,
                age: 0,
                active: false,
                note: 0.0,
            };
        }
    }

    /// Allocate a voice for the given note.
    /// Mono mode: always returns voice 0.
    /// Poly mode: prefers the oldest inactive voice, or steals the oldest active one.
    /// This avoids immediately reusing a voice that just entered release when
    /// there are other inactive voices available.
    pub fn allocate_voice(&mut self, note: f32) -> &mut VoiceSlot {
        self.age_counter += 1;

        if !self.polyphonic || self.num_voices <= 1 {
            // Mono: always voice 0
            let slot = &mut self.voices[0];
            slot.age = self.age_counter;
            slot.active = true;
            slot.note = note;
            return slot;
        }

        // Poly: prefer the oldest inactive voice so freshly released voices are
        // only reused after longer-idle voices have been consumed.
        let mut free_idx = None;
        let mut free_age = u64::MAX;
        let mut oldest_idx = 0;
        let mut oldest_age = u64::MAX;

        for i in 0..self.num_voices {
            if !self.voices[i].active {
                if self.voices[i].age < free_age {
                    free_idx = Some(i);
                    free_age = self.voices[i].age;
                }
            }
            if self.voices[i].age < oldest_age {
                oldest_age = self.voices[i].age;
                oldest_idx = i;
            }
        }

        let idx = free_idx.unwrap_or(oldest_idx);
        let slot = &mut self.voices[idx];
        slot.age = self.age_counter;
        slot.active = true;
        slot.note = note;
        slot
    }

    /// Allocate a voice for samplers.
    /// In poly mode, a repeated note should retrigger the existing voice instead
    /// of layering another copy of the same sample.
    pub fn allocate_voice_retriggering_same_note(&mut self, note: f32) -> &mut VoiceSlot {
        if self.polyphonic && self.num_voices > 1 {
            for i in 0..self.num_voices {
                if self.voices[i].active && (self.voices[i].note - note).abs() < 0.01 {
                    self.age_counter += 1;
                    let slot = &mut self.voices[i];
                    slot.age = self.age_counter;
                    slot.active = true;
                    slot.note = note;
                    return slot;
                }
            }
        }

        self.allocate_voice(note)
    }

    pub fn allocate_voice_retriggering_same_note_with_limit(
        &mut self,
        note: f32,
        max_polyphony: usize,
    ) -> &mut VoiceSlot {
        let max_polyphony = max_polyphony.max(1).min(self.num_voices.max(1));
        if max_polyphony > 1 && self.num_voices > 1 {
            for i in 0..self.num_voices {
                if self.voices[i].active && (self.voices[i].note - note).abs() < 0.01 {
                    self.age_counter += 1;
                    let slot = &mut self.voices[i];
                    slot.age = self.age_counter;
                    slot.active = true;
                    slot.note = note;
                    return slot;
                }
            }
        }

        self.age_counter += 1;
        let mut active_count = 0usize;
        let mut oldest_active_idx = 0usize;
        let mut oldest_active_age = u64::MAX;
        let mut oldest_idle_idx = None;
        let mut oldest_idle_age = u64::MAX;

        for i in 0..self.num_voices {
            let voice = &self.voices[i];
            if voice.active {
                active_count += 1;
                if voice.age < oldest_active_age {
                    oldest_active_idx = i;
                    oldest_active_age = voice.age;
                }
            } else if voice.age < oldest_idle_age {
                oldest_idle_idx = Some(i);
                oldest_idle_age = voice.age;
            }
        }

        let idx = if active_count >= max_polyphony {
            oldest_active_idx
        } else {
            oldest_idle_idx.unwrap_or(oldest_active_idx)
        };
        let slot = &mut self.voices[idx];
        slot.age = self.age_counter;
        slot.active = true;
        slot.note = note;
        slot
    }

    pub fn release_voice_by_note(&mut self, note: f32) {
        for i in 0..self.num_voices {
            if self.voices[i].active && (self.voices[i].note - note).abs() < 0.01 {
                self.voices[i].active = false;
                return;
            }
        }
    }

    pub fn release_voice_by_logical_id(&mut self, logical_id: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                return;
            }
        }
    }

    pub fn all_logical_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.voices[..self.num_voices].iter().map(|v| v.logical_id)
    }
}

#[cfg(test)]
mod tests {
    use super::VoicePool;

    #[test]
    fn prefers_long_idle_inactive_voices_over_recently_released_ones() {
        let mut pool = VoicePool::new();
        pool.polyphonic = true;
        for lid in 1..=12 {
            pool.add_voice(lid, lid as i32);
        }

        assert_eq!(pool.allocate_voice(0.0).logical_id, 1);
        assert_eq!(pool.allocate_voice(4.0).logical_id, 2);

        pool.release_voice_by_note(0.0);
        pool.release_voice_by_note(4.0);

        assert_eq!(pool.allocate_voice(7.0).logical_id, 3);
        assert_eq!(pool.allocate_voice(11.0).logical_id, 4);
    }

    #[test]
    fn sampler_allocation_retriggers_active_same_note_in_poly_mode() {
        let mut pool = VoicePool::new();
        pool.polyphonic = true;
        for lid in 1..=4 {
            pool.add_voice(lid, lid as i32);
        }

        let first = pool.allocate_voice_retriggering_same_note(0.0).logical_id;
        let second = pool.allocate_voice_retriggering_same_note(7.0).logical_id;
        let repeated = pool.allocate_voice_retriggering_same_note(0.0).logical_id;

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(repeated, first);
    }

    #[test]
    fn limited_sampler_allocation_respects_max_polyphony() {
        let mut pool = VoicePool::new();
        for lid in 1..=4 {
            pool.add_voice(lid, lid as i32);
        }

        assert_eq!(
            pool.allocate_voice_retriggering_same_note_with_limit(0.0, 2)
                .logical_id,
            1
        );
        assert_eq!(
            pool.allocate_voice_retriggering_same_note_with_limit(4.0, 2)
                .logical_id,
            2
        );
        assert_eq!(
            pool.allocate_voice_retriggering_same_note_with_limit(7.0, 2)
                .logical_id,
            1
        );
        assert_eq!(
            pool.voices[..pool.num_voices]
                .iter()
                .filter(|voice| voice.active)
                .count(),
            2
        );
    }
}
