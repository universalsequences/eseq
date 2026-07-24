//! Take entities (docs/takes-and-additive-arrangement-recording-spec.md 6.1).
//!
//! A take is a thin ownership/indexing layer over ordinary pool patterns: an
//! ordered list of chunk `PatternId`s claimed from the owning track's
//! `TrackPatternPool`, plus a playable length. Chunks are exclusive to one
//! take, never referenced by scene cells, and hidden from the mixer clip
//! grid — "hidden" and "non-looping" are derived from ownership, never flags
//! on `TrackPatternData` (locked decision).

use super::*;

/// Stable per-track take identity. Allocated monotonically from
/// `TrackTakePool::next_take_id`, never reused within a project.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct TakeId(pub u64);

/// A linear, non-looping recording on one track (takes spec 6.1).
#[derive(Clone, Debug, PartialEq)]
pub struct TrackTake {
    pub id: TakeId,
    /// "Take {n}", user-renamable.
    pub name: String,
    /// Ordered chunk patterns in this track's pool. Invariant: every chunk
    /// except the last covers `MAX_STEPS` steps; chunks are exclusive to one
    /// take and never referenced by any scene cell.
    pub chunks: Vec<PatternId>,
    /// Playable length in steps. The lane is silent (never wraps) past this.
    pub total_len_steps: u32,
}

impl TrackTake {
    /// Chunk index and chunk-local step for clip-local position `p` (steps),
    /// or `None` when `p` is at/past the take end (silent, spec 6.1).
    pub fn chunk_step_at(&self, p: f64) -> Option<(usize, f64)> {
        if !(p >= 0.0) || p >= self.total_len_steps as f64 {
            return None;
        }
        let chunk = (p / MAX_STEPS as f64).floor() as usize;
        (chunk < self.chunks.len()).then(|| (chunk, p - (chunk * MAX_STEPS) as f64))
    }
}

/// Per-track take pool, stored alongside the pattern pool on `ProjectScenes`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackTakePool {
    /// Small; linear scan by id is fine.
    pub takes: Vec<TrackTake>,
    /// Monotonic per track, never reused. 0 is a valid first id.
    pub next_take_id: u64,
}

impl TrackTakePool {
    pub fn get(&self, id: TakeId) -> Option<&TrackTake> {
        self.takes.iter().find(|take| take.id == id)
    }

    pub fn get_mut(&mut self, id: TakeId) -> Option<&mut TrackTake> {
        self.takes.iter_mut().find(|take| take.id == id)
    }

    pub fn contains(&self, id: TakeId) -> bool {
        self.get(id).is_some()
    }

    /// Every pool pattern id claimed as a chunk by some take. Claimed
    /// patterns are hidden from the mixer clip grid (spec 6.1/11.2).
    pub fn claimed(&self) -> impl Iterator<Item = PatternId> + '_ {
        self.takes.iter().flat_map(|take| take.chunks.iter().copied())
    }

    pub fn is_claimed(&self, id: PatternId) -> bool {
        self.claimed().any(|chunk| chunk == id)
    }

    /// Register a new take over already-inserted chunk patterns and return
    /// its id. The caller owns chunk minting (`TrackPatternPool::insert`)
    /// and invariant upkeep (spec 6.3).
    pub fn insert(&mut self, name: Option<String>, chunks: Vec<PatternId>, total_len_steps: u32) -> TakeId {
        let id = TakeId(self.next_take_id);
        self.next_take_id = self.next_take_id.saturating_add(1);
        let name = name.unwrap_or_else(|| format!("Take {}", id.0 + 1));
        self.takes.push(TrackTake {
            id,
            name,
            chunks,
            total_len_steps,
        });
        id
    }

    /// Remove a take, returning it (its chunk ids) so the caller can drop
    /// the chunk patterns from the pattern pool and clean up song overrides
    /// (spec 6.4: one undo entry, handled at the app layer).
    pub fn remove(&mut self, id: TakeId) -> Option<TrackTake> {
        let idx = self.takes.iter().position(|take| take.id == id)?;
        Some(self.takes.remove(idx))
    }
}

/// Take invariants (takes spec 6.3), checked per track. `scene_cells_hold`
/// answers whether any scene cell of this track references a pattern id.
pub fn validate_track_take_pool(
    track: usize,
    take_pool: &TrackTakePool,
    pattern_pool: &TrackPatternPool,
    scene_cell_ids: &HashSet<PatternId>,
) -> Result<(), String> {
    let mut seen_ids = HashSet::new();
    let mut seen_chunks = HashSet::new();
    for take in &take_pool.takes {
        if !seen_ids.insert(take.id) {
            return Err(format!(
                "Track {} reuses take id {}; take ids must be unique",
                track + 1,
                take.id.0
            ));
        }
        if take.id.0 >= take_pool.next_take_id {
            return Err(format!(
                "Track {} take id {} is not below the allocator ({})",
                track + 1,
                take.id.0,
                take_pool.next_take_id
            ));
        }
        if take.chunks.is_empty() {
            return Err(format!(
                "Track {} take {} has no chunk patterns",
                track + 1,
                take.id.0
            ));
        }
        if take.total_len_steps as usize > take.chunks.len() * MAX_STEPS {
            return Err(format!(
                "Track {} take {} claims {} steps but its {} chunk(s) cover at most {}",
                track + 1,
                take.id.0,
                take.total_len_steps,
                take.chunks.len(),
                take.chunks.len() * MAX_STEPS
            ));
        }
        for chunk in &take.chunks {
            if !pattern_pool.contains(*chunk) {
                return Err(format!(
                    "Track {} take {} references chunk pattern {} which is not in the \
                     track's pattern pool",
                    track + 1,
                    take.id.0,
                    chunk.0
                ));
            }
            if !seen_chunks.insert(*chunk) {
                return Err(format!(
                    "Track {} chunk pattern {} is claimed by more than one take",
                    track + 1,
                    chunk.0
                ));
            }
            if scene_cell_ids.contains(chunk) {
                return Err(format!(
                    "Track {} chunk pattern {} is referenced by a scene cell; take chunks \
                     must be exclusive to their take",
                    track + 1,
                    chunk.0
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_with(patterns: &[u64]) -> TrackPatternPool {
        let mut pool = TrackPatternPool::default();
        for _ in patterns {
            let data = PatternSnapshot::new_default(1, &[])
                .track_pattern_data(0)
                .expect("default track data");
            pool.insert(data);
        }
        pool
    }

    #[test]
    fn take_pool_insert_allocates_monotonic_ids_and_names() {
        let mut takes = TrackTakePool::default();
        let a = takes.insert(None, vec![PatternId(1)], 100);
        let b = takes.insert(Some("Riff".into()), vec![PatternId(2)], 12);
        assert_eq!(a, TakeId(0));
        assert_eq!(b, TakeId(1));
        assert_eq!(takes.next_take_id, 2);
        assert_eq!(takes.get(a).unwrap().name, "Take 1");
        assert_eq!(takes.get(b).unwrap().name, "Riff");
        // Removal never frees an id.
        takes.remove(a).expect("removed");
        let c = takes.insert(None, vec![PatternId(3)], 8);
        assert_eq!(c, TakeId(2));
    }

    #[test]
    fn claimed_reports_every_chunk_across_takes() {
        let mut takes = TrackTakePool::default();
        takes.insert(None, vec![PatternId(1), PatternId(2)], 512);
        takes.insert(None, vec![PatternId(5)], 64);
        let claimed: HashSet<PatternId> = takes.claimed().collect();
        assert_eq!(
            claimed,
            [PatternId(1), PatternId(2), PatternId(5)].into_iter().collect()
        );
        assert!(takes.is_claimed(PatternId(2)));
        assert!(!takes.is_claimed(PatternId(3)));
    }

    #[test]
    fn chunk_step_at_resolves_boundaries_and_silences_past_end() {
        let take = TrackTake {
            id: TakeId(0),
            name: "Take 1".into(),
            chunks: vec![PatternId(1), PatternId(2)],
            total_len_steps: MAX_STEPS as u32 + 40,
        };
        assert_eq!(take.chunk_step_at(0.0), Some((0, 0.0)));
        assert_eq!(take.chunk_step_at(MAX_STEPS as f64 - 0.5), Some((0, MAX_STEPS as f64 - 0.5)));
        // Exactly on the chunk boundary: chunk 1 step 0 (no wrap, no gap).
        assert_eq!(take.chunk_step_at(MAX_STEPS as f64), Some((1, 0.0)));
        assert_eq!(take.chunk_step_at(MAX_STEPS as f64 + 39.25), Some((1, 39.25)));
        // At and past the end: silent, never wrapped.
        assert_eq!(take.chunk_step_at(MAX_STEPS as f64 + 40.0), None);
        assert_eq!(take.chunk_step_at(10_000.0), None);
        assert_eq!(take.chunk_step_at(-1.0), None);
    }

    #[test]
    fn validate_rejects_broken_take_invariants() {
        let pattern_pool = pool_with(&[1, 2, 3]);
        let mut scene_ids = HashSet::new();

        let mut takes = TrackTakePool::default();
        takes.insert(None, vec![PatternId(1)], 10);
        validate_track_take_pool(0, &takes, &pattern_pool, &scene_ids).expect("valid");

        // Chunk missing from the pattern pool.
        let mut broken = takes.clone();
        broken.takes[0].chunks = vec![PatternId(9)];
        let err = validate_track_take_pool(0, &broken, &pattern_pool, &scene_ids).unwrap_err();
        assert!(err.contains("not in the"), "{err}");

        // Empty chunk list.
        let mut broken = takes.clone();
        broken.takes[0].chunks.clear();
        let err = validate_track_take_pool(0, &broken, &pattern_pool, &scene_ids).unwrap_err();
        assert!(err.contains("no chunk"), "{err}");

        // Length exceeding chunk coverage.
        let mut broken = takes.clone();
        broken.takes[0].total_len_steps = MAX_STEPS as u32 + 1;
        let err = validate_track_take_pool(0, &broken, &pattern_pool, &scene_ids).unwrap_err();
        assert!(err.contains("cover at most"), "{err}");

        // Chunk shared between two takes.
        let mut broken = takes.clone();
        broken.insert(None, vec![PatternId(1)], 5);
        let err = validate_track_take_pool(0, &broken, &pattern_pool, &scene_ids).unwrap_err();
        assert!(err.contains("more than one take"), "{err}");

        // Chunk referenced by a scene cell.
        scene_ids.insert(PatternId(1));
        let err = validate_track_take_pool(0, &takes, &pattern_pool, &scene_ids).unwrap_err();
        assert!(err.contains("scene cell"), "{err}");
    }
}
