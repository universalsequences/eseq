//! Dependencies on host-published paint data. Versions are observed while
//! holding the same lock as the data read: a publication racing a painter
//! remains pending for its next frame. No publisher needs to know widget IDs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(super) fn generation() -> u64 { GENERATION.load(Ordering::Relaxed) }

#[derive(Default)]
pub(super) struct PaintDependencies(Vec<(Arc<AtomicU64>, u64)>);

thread_local! {
    static READS: RefCell<Vec<PaintDependencies>> = const { RefCell::new(Vec::new()) };
}

impl PaintDependencies {
    pub(super) fn is_empty(&self) -> bool { self.0.is_empty() }

    pub(super) fn changed(&self) -> bool {
        self.0.iter().any(|(revision, observed)| revision.load(Ordering::Relaxed) != *observed)
    }

    pub(super) fn capture<T>(paint: impl FnOnce() -> T) -> (T, Self) {
        struct Capture;
        impl Drop for Capture {
            fn drop(&mut self) { READS.with(|reads| { reads.borrow_mut().pop(); }); }
        }
        READS.with(|reads| reads.borrow_mut().push(Self::default()));
        let guard = Capture;
        let result = paint();
        let dependencies = READS.with(|reads| std::mem::take(reads.borrow_mut().last_mut().unwrap()));
        drop(guard);
        (result, dependencies)
    }

    fn observe(revision: &Arc<AtomicU64>) {
        READS.with(|reads| {
            for capture in reads.borrow_mut().iter_mut() {
                if !capture.0.iter().any(|(old, _)| Arc::ptr_eq(old, revision)) {
                    capture.0.push((Arc::clone(revision), revision.load(Ordering::Relaxed)));
                }
            }
        });
    }
}

struct Entry<T> {
    value: Option<T>,
    revision: Arc<AtomicU64>,
}

impl<T> Default for Entry<T> {
    fn default() -> Self { Self { value: None, revision: Arc::new(AtomicU64::new(0)) } }
}

impl<T> Entry<T> {
    fn change(&mut self, value: Option<T>) {
        self.value = value;
        self.revision.fetch_add(1, Ordering::Relaxed);
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct PaintResourceStore<T>(Mutex<HashMap<String, Entry<T>>>);

impl<T> Default for PaintResourceStore<T> {
    fn default() -> Self { Self(Mutex::new(HashMap::new())) }
}

impl<T: Clone> PaintResourceStore<T> {
    pub(crate) fn get(&self, key: &str) -> Option<T> {
        let mut entries = self.0.lock().unwrap();
        if let Some(entry) = entries.get(key) {
            PaintDependencies::observe(&entry.revision);
            return entry.value.clone();
        }
        if READS.with(|reads| reads.borrow().is_empty()) { return None; }
        let entry = entries.entry(key.to_string()).or_default();
        PaintDependencies::observe(&entry.revision);
        entry.value.clone()
    }

    pub(crate) fn publish(&self, key: String, value: T) {
        self.publish_many([(key, value)]);
    }

    pub(crate) fn publish_many(&self, values: impl IntoIterator<Item = (String, T)>) {
        let mut entries = self.0.lock().unwrap();
        for (key, value) in values { entries.entry(key).or_default().change(Some(value)); }
    }

    pub(crate) fn retain(&self, mut keep: impl FnMut(&str) -> bool) {
        self.0.lock().unwrap().retain(|key, entry| {
            if entry.value.is_some() && !keep(key) { entry.change(None); }
            entry.value.is_some() || Arc::strong_count(&entry.revision) > 1
        });
    }

    pub(crate) fn clear(&self) { self.retain(|_| false); }
}

impl PaintResourceStore<bool> {
    pub(crate) fn replace_set(&self, prefix: &str, keys: &std::collections::HashSet<String>) {
        let mut entries = self.0.lock().unwrap();
        entries.retain(|key, entry| {
            if key.starts_with(prefix) && !keys.contains(key) && entry.value.is_some() {
                entry.change(None);
            }
            entry.value.is_some() || Arc::strong_count(&entry.revision) > 1
        });
        for key in keys {
            let entry = entries.entry(key.clone()).or_default();
            if entry.value != Some(true) { entry.change(Some(true)); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_reads_missing_values_removals_and_racing_publications() {
        let store = PaintResourceStore::default();
        let (_, missing) = PaintDependencies::capture(|| assert_eq!(store.get("a"), None));
        store.publish("b".into(), 2);
        assert!(!missing.changed());
        store.publish("a".into(), 1);
        assert!(missing.changed());
        let (_, read) = PaintDependencies::capture(|| assert_eq!(store.get("a"), Some(1)));
        store.retain(|key| key == "b");
        assert!(read.changed());
        let (_, race) = PaintDependencies::capture(|| {
            assert_eq!(store.get("a"), None);
            store.publish("a".into(), 3);
        });
        assert!(race.changed(), "publication after the read must remain pending");
    }
}
