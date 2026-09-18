use std::ops::{Deref, DerefMut};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError};
use std::sync::atomic::{AtomicU64, Ordering};

/// A model revision follows mutable access, including replacement and undo.
/// Keeping it with the lock avoids relying on every editor/loader to remember
/// an unrelated invalidation call. Reads neither clone the model nor advance
/// the revision; writes may conservatively advance it even if values compare
/// equal. The bump happens before the guard releases the model lock.
pub struct RevisionedMutex<T> {
    value: Mutex<T>,
    revision: AtomicU64,
}

impl<T> RevisionedMutex<T> {
    pub fn new(value: T) -> Self {
        Self { value: Mutex::new(value), revision: AtomicU64::new(0) }
    }

    pub fn revision(&self) -> u64 { self.revision.load(Ordering::Acquire) }

    pub fn lock(&self) -> LockResult<RevisionedGuard<'_, T>> {
        let wrap = |value| RevisionedGuard { value, revision: &self.revision, written: false };
        match self.value.lock() {
            Ok(value) => Ok(wrap(value)),
            Err(error) => Err(PoisonError::new(wrap(error.into_inner()))),
        }
    }
}

pub struct RevisionedGuard<'a, T> {
    value: MutexGuard<'a, T>,
    revision: &'a AtomicU64,
    written: bool,
}

impl<T> Deref for RevisionedGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T { &self.value }
}

impl<T> DerefMut for RevisionedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { self.written = true; &mut self.value }
}

impl<T> Drop for RevisionedGuard<'_, T> {
    fn drop(&mut self) {
        if self.written { self.revision.fetch_add(1, Ordering::Release); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_free_and_mutable_access_including_replacement_advances_revision() {
        let model = RevisionedMutex::new(vec![1]);
        assert_eq!(model.lock().unwrap()[0], 1);
        assert_eq!(model.revision(), 0);
        model.lock().unwrap()[0] = 2;
        assert_eq!(model.revision(), 1);
        *model.lock().unwrap() = vec![1];
        assert_eq!(model.revision(), 2, "undo must not reuse a previous revision");
    }
}
