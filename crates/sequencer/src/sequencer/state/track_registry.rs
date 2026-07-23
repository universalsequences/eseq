use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackRegistryError {
    InvalidId(TrackId),
    DuplicateId(TrackId),
    IndexOutOfRange { index: usize, len: usize },
    IdExhausted,
}

/// Bidirectional mapping between stable track ids and dense runtime indices.
#[derive(Clone, Debug)]
pub struct TrackRegistry {
    order: Vec<TrackId>,
    index_by_id: HashMap<TrackId, usize>,
    next_id: u64,
}

impl Default for TrackRegistry {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            index_by_id: HashMap::new(),
            next_id: TrackId::MIN.0,
        }
    }
}

impl TrackRegistry {
    pub fn for_legacy_track_count(track_count: usize) -> Result<Self, TrackRegistryError> {
        let count = u64::try_from(track_count).map_err(|_| TrackRegistryError::IdExhausted)?;
        if count == u64::MAX {
            return Err(TrackRegistryError::IdExhausted);
        }
        Self::from_ids((1..=count).map(TrackId))
    }

    pub fn from_ids(ids: impl IntoIterator<Item = TrackId>) -> Result<Self, TrackRegistryError> {
        let order = ids.into_iter().collect::<Vec<_>>();
        let mut index_by_id = HashMap::with_capacity(order.len());
        let mut max_id = 0u64;
        for (index, id) in order.iter().copied().enumerate() {
            if id.0 == 0 {
                return Err(TrackRegistryError::InvalidId(id));
            }
            if index_by_id.insert(id, index).is_some() {
                return Err(TrackRegistryError::DuplicateId(id));
            }
            max_id = max_id.max(id.0);
        }
        let next_id = max_id.checked_add(1).unwrap_or(0);
        Ok(Self {
            order,
            index_by_id,
            next_id,
        })
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn ids(&self) -> &[TrackId] {
        &self.order
    }

    pub fn id_at(&self, index: usize) -> Option<TrackId> {
        self.order.get(index).copied()
    }

    pub fn index_of(&self, id: TrackId) -> Option<usize> {
        self.index_by_id.get(&id).copied()
    }

    pub fn can_allocate(&self) -> bool {
        self.next_id != 0
    }

    pub fn allocate_at(&mut self, index: usize) -> Result<TrackId, TrackRegistryError> {
        if index > self.order.len() {
            return Err(TrackRegistryError::IndexOutOfRange {
                index,
                len: self.order.len(),
            });
        }
        let id = TrackId::new(self.next_id).ok_or(TrackRegistryError::IdExhausted)?;
        self.next_id = self.next_id.checked_add(1).unwrap_or(0);
        self.order.insert(index, id);
        self.reindex_from(index);
        Ok(id)
    }

    pub fn allocate(&mut self) -> Result<TrackId, TrackRegistryError> {
        self.allocate_at(self.order.len())
    }

    pub fn remove(&mut self, id: TrackId) -> Option<usize> {
        let index = self.index_by_id.remove(&id)?;
        self.order.remove(index);
        self.reindex_from(index);
        Some(index)
    }

    pub fn replace_at(
        &mut self,
        index: usize,
        replacement: TrackId,
    ) -> Result<TrackId, TrackRegistryError> {
        if replacement.0 == 0 {
            return Err(TrackRegistryError::InvalidId(replacement));
        }
        let Some(current) = self.order.get(index).copied() else {
            return Err(TrackRegistryError::IndexOutOfRange {
                index,
                len: self.order.len(),
            });
        };
        if current == replacement {
            return Ok(current);
        }
        if self.index_by_id.contains_key(&replacement) {
            return Err(TrackRegistryError::DuplicateId(replacement));
        }
        self.order[index] = replacement;
        self.index_by_id.remove(&current);
        self.index_by_id.insert(replacement, index);
        if replacement.0 >= self.next_id && self.next_id != 0 {
            self.next_id = replacement.0.checked_add(1).unwrap_or(0);
        }
        Ok(current)
    }

    pub fn move_to(&mut self, id: TrackId, target: usize) -> Result<(), TrackRegistryError> {
        if target >= self.order.len() {
            return Err(TrackRegistryError::IndexOutOfRange {
                index: target,
                len: self.order.len(),
            });
        }
        let source = self
            .index_of(id)
            .ok_or(TrackRegistryError::InvalidId(id))?;
        if source == target {
            return Ok(());
        }
        self.order.remove(source);
        self.order.insert(target, id);
        self.reindex_from(source.min(target));
        Ok(())
    }

    fn reindex_from(&mut self, start: usize) {
        for (index, id) in self.order.iter().copied().enumerate().skip(start) {
            self.index_by_id.insert(id, index);
        }
    }
}

#[cfg(test)]
mod track_registry_tests {
    use super::{TrackId, TrackRegistry, TrackRegistryError};

    #[test]
    fn stable_track_ids_resolve_after_dense_reordering_and_deletion() {
        let mut registry = TrackRegistry::default();
        let first = registry.allocate().expect("allocate first track id");
        let second = registry.allocate().expect("allocate second track id");
        let third = registry.allocate().expect("allocate third track id");

        registry.move_to(third, 0).expect("move third track");
        assert_eq!(registry.ids(), &[third, first, second]);
        assert_eq!(registry.index_of(first), Some(1));
        assert_eq!(registry.index_of(third), Some(0));

        assert_eq!(registry.remove(first), Some(1));
        assert_eq!(registry.ids(), &[third, second]);
        assert_eq!(registry.index_of(second), Some(1));
        assert_eq!(registry.index_of(first), None);
    }

    #[test]
    fn imported_track_ids_are_validated_and_new_ids_never_reuse_existing_values() {
        assert!(matches!(
            TrackRegistry::from_ids([TrackId(1), TrackId(1)]),
            Err(TrackRegistryError::DuplicateId(TrackId(1)))
        ));
        assert!(matches!(
            TrackRegistry::from_ids([TrackId(0)]),
            Err(TrackRegistryError::InvalidId(TrackId(0)))
        ));

        let mut registry = TrackRegistry::from_ids([TrackId(8), TrackId(3)])
            .expect("import unique nonzero ids");
        assert_eq!(registry.allocate().expect("allocate after import"), TrackId(9));

        let legacy = TrackRegistry::for_legacy_track_count(3)
            .expect("assign deterministic ids to legacy tracks");
        assert_eq!(legacy.ids(), &[TrackId(1), TrackId(2), TrackId(3)]);
    }
}
