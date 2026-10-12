use std::ops::Deref;
use std::sync::Arc;

use crate::sequencer::MAX_STEPS;

/// Stored step locks with constant-time indexed reads and copy-on-write edits.
///
/// Missing rows/cells mean no lock. Only rows containing locks allocate cells;
/// trailing empty cells and rows are omitted. An entirely empty grid allocates
/// nothing. Cloning shares the prepared rows, so playback never expands them.
/// Mutation is explicit to keep these invariants true at every authoring entry.
#[derive(Clone, Debug, PartialEq)]
pub struct StepLockRows<T>(Option<Arc<Vec<Vec<Option<T>>>>>);

impl<T> Default for StepLockRows<T> {
    fn default() -> Self { Self(None) }
}

impl<T> Deref for StepLockRows<T> {
    type Target = [Vec<Option<T>>];

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl<'a, T> IntoIterator for &'a StepLockRows<T> {
    type Item = &'a Vec<Option<T>>;
    type IntoIter = std::slice::Iter<'a, Vec<Option<T>>>;

    fn into_iter(self) -> Self::IntoIter { self.iter() }
}

impl<T> From<Vec<Vec<Option<T>>>> for StepLockRows<T> {
    fn from(mut rows: Vec<Vec<Option<T>>>) -> Self {
        rows.truncate(MAX_STEPS);
        for row in &mut rows {
            row.truncate(row.iter().rposition(Option::is_some).map_or(0, |i| i + 1));
            row.shrink_to_fit();
        }
        rows.truncate(rows.iter().rposition(|row| !row.is_empty()).map_or(0, |i| i + 1));
        if rows.is_empty() { return Self::default(); }
        rows.shrink_to_fit();
        Self(Some(Arc::new(rows)))
    }
}

impl<T> FromIterator<Vec<Option<T>>> for StepLockRows<T> {
    fn from_iter<I: IntoIterator<Item = Vec<Option<T>>>>(rows: I) -> Self {
        rows.into_iter().take(MAX_STEPS).collect::<Vec<_>>().into()
    }
}

impl<T> StepLockRows<T> {
    pub fn clear(&mut self) { self.0 = None; }

    pub fn cell(&self, step: usize, param: usize) -> Option<&T> {
        self.get(step)?.get(param)?.as_ref()
    }

    /// Includes the shared allocation in full (a conservative per-owner count).
    pub fn retained_bytes(&self) -> usize {
        self.0.as_ref().map_or(0, |rows| {
            2 * std::mem::size_of::<usize>() + std::mem::size_of_val(rows.as_ref())
                + rows.capacity() * std::mem::size_of::<Vec<Option<T>>>()
                + rows.iter().map(|row| row.capacity() * std::mem::size_of::<Option<T>>()).sum::<usize>()
        })
    }
}

impl<T: Clone> StepLockRows<T> {
    pub fn for_each_value_mut(&mut self, mut edit: impl FnMut(&mut T)) {
        let Some(rows) = self.0.as_mut() else { return; };
        for value in Arc::make_mut(rows).iter_mut().flatten().flatten() { edit(value); }
    }

    pub fn set(&mut self, step: usize, param: usize, value: Option<T>) {
        assert!(step < MAX_STEPS, "step lock outside pattern capacity");
        if value.is_none() && self.cell(step, param).is_none() { return; }
        let rows = Arc::make_mut(self.0.get_or_insert_with(|| Arc::new(Vec::new())));
        rows.resize_with(rows.len().max(step + 1), Vec::new);
        let row = &mut rows[step];
        row.resize_with(row.len().max(param + 1), || None);
        row[param] = value;
        row.truncate(row.iter().rposition(Option::is_some).map_or(0, |i| i + 1));
        if row.is_empty() { *row = Vec::new(); }
        rows.truncate(rows.iter().rposition(|row| !row.is_empty()).map_or(0, |i| i + 1));
        if rows.is_empty() { self.0 = None; }
    }

    pub fn clear_step(&mut self, step: usize) {
        if !self.get(step).is_some_and(|row| !row.is_empty()) { return; }
        let rows = Arc::make_mut(self.0.as_mut().unwrap());
        rows[step] = Vec::new();
        rows.truncate(rows.iter().rposition(|row| !row.is_empty()).map_or(0, |i| i + 1));
        if rows.is_empty() { self.0 = None; }
    }

    pub fn truncate_params(&mut self, num_params: usize) {
        if self.iter().all(|row| row.len() <= num_params) { return; }
        let mut rows = self.to_vec();
        for row in &mut rows { row.truncate(num_params); }
        *self = rows.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_rows_share_until_edited_and_release_empty_storage() {
        let mut rows = StepLockRows::<f32>::from(vec![vec![None; 512]; MAX_STEPS]);
        assert_eq!(rows.retained_bytes(), 0);
        rows.set(255, 511, Some(0.75));
        let original = rows.clone();
        assert!(Arc::ptr_eq(rows.0.as_ref().unwrap(), original.0.as_ref().unwrap()));
        rows.set(3, 1, Some(0.25));
        assert_eq!(original.cell(3, 1), None);
        assert_eq!(original.cell(255, 511), Some(&0.75));
        rows.clear_step(255);
        assert_eq!(rows.len(), 4);
        rows.set(3, 1, None);
        assert_eq!(rows.retained_bytes(), 0);
        assert_eq!(original.cell(255, 511), Some(&0.75));
    }

    #[test]
    fn conversion_and_descriptor_shrink_preserve_only_present_cells() {
        let mut rows = StepLockRows::from(vec![vec![None, Some(0.0), None], vec![None; 4]]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 2);
        rows.truncate_params(1);
        assert_eq!(rows.retained_bytes(), 0);
    }
}
