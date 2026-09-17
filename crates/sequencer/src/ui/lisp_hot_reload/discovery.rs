//! Filesystem discovery lives on this worker, never on the event/render loop.
//! Native events update the affected part of the index. Only startup and lost
//! events require a full reconciliation; idle time performs no filesystem work.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::{watch_path, DEBOUNCE_WINDOW};

#[derive(Default, Debug)]
pub(crate) struct ReloadBatch {
    pub paths: BTreeSet<PathBuf>,
    // Includes removed sources: classification must survive file deletion.
    pub custom_ui: BTreeSet<PathBuf>,
}

impl ReloadBatch {
    pub fn is_empty(&self) -> bool { self.paths.is_empty() }
    fn custom(&mut self, path: PathBuf) {
        self.paths.insert(path.clone());
        self.custom_ui.insert(path);
    }
}

#[derive(Default)]
struct Pending {
    sources: Option<Vec<PathBuf>>,
    events: Vec<Event>,
    rescan: bool,
    stop: bool,
    first_event: Option<Instant>,
    last_event: Option<Instant>,
}

#[derive(Default)]
struct Mailbox {
    pending: Mutex<Pending>,
    wake: Condvar,
    ready: Mutex<ReloadBatch>,
    #[cfg(test)]
    probe: Mutex<WorkerProbe>,
}

#[cfg(test)]
#[derive(Default, Clone)]
struct WorkerProbe {
    passes: usize,
    directories_scanned: usize,
    custom: BTreeSet<PathBuf>,
    sources: BTreeSet<PathBuf>,
}

impl Mailbox {
    fn event(&self, result: notify::Result<Event>) {
        let mut pending = self.pending.lock().unwrap();
        match result {
            Ok(event) if matches!(event.kind, EventKind::Access(_)) && !event.need_rescan() => return,
            Ok(event) if !event.need_rescan() && !pending.rescan && pending.events.len() < 1024 => {
                pending.events.push(event);
            }
            result => {
                if let Err(error) = result { eprintln!("metal_seq: Lisp watcher error: {error}"); }
                // Bounded ingress. Overflow/OS loss is an explicit reconciliation,
                // never a silent lost change or an unbounded queue of events.
                pending.events.clear();
                pending.rescan = true;
            }
        }
        let now = Instant::now();
        pending.first_event.get_or_insert(now);
        pending.last_event = Some(now);
        self.wake.notify_one();
    }

    fn next(&self) -> Pending {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if pending.stop { return std::mem::take(&mut *pending); }
            if let Some(sources) = pending.sources.take() {
                // Updating subscriptions must not flush a half-written file's
                // pending event before its debounce window has elapsed.
                return Pending { sources: Some(sources), ..Default::default() };
            }
            if let (Some(first), Some(last)) = (pending.first_event, pending.last_event) {
                // Continuous writes must not postpone reloading indefinitely.
                let deadline = (last + DEBOUNCE_WINDOW).min(first + Duration::from_secs(1));
                if Instant::now() >= deadline { return std::mem::take(&mut *pending); }
                pending = self.wake.wait_timeout(pending, deadline.saturating_duration_since(Instant::now())).unwrap().0;
            } else {
                pending = self.wake.wait(pending).unwrap();
            }
        }
    }

    fn publish(&self, batch: ReloadBatch) {
        let mut ready = self.ready.lock().unwrap();
        ready.paths.extend(batch.paths);
        ready.custom_ui.extend(batch.custom_ui);
    }
}

pub(super) struct DiscoveryWorker {
    mailbox: Arc<Mailbox>,
    thread: Option<JoinHandle<()>>,
}

impl DiscoveryWorker {
    pub fn start(sources: Vec<PathBuf>, roots: impl Fn() -> DiscoveryRoots + Send + 'static) -> std::io::Result<Self> {
        let mailbox = Arc::new(Mailbox::default());
        mailbox.pending.lock().unwrap().sources = Some(sources);
        let shared = Arc::clone(&mailbox);
        let thread = std::thread::Builder::new().name("lisp-ui-discovery".into()).spawn(move || {
            let callback = Arc::clone(&shared);
            let mut watcher = match RecommendedWatcher::new(move |event| callback.event(event), Config::default()) {
                Ok(watcher) => watcher,
                Err(error) => {
                    eprintln!("metal_seq: cannot start Lisp watcher: {error}");
                    return;
                }
            };
            let mut index = DiscoveryIndex::new(roots());
            let mut watches = BTreeMap::new();
            loop {
                let pending = shared.next();
                if pending.stop { break; }
                if let Some(sources) = pending.sources { index.set_sources(sources); }
                // inotify removes watches on directory deletion; a rename-away
                // followed by replacement can leave the same path in our plan.
                let stale: Vec<_> = watches.keys().filter(|watched: &&PathBuf| pending.rescan
                    || pending.events.iter().any(|event| {
                        matches!(event.kind, EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_)))
                            && event.paths.iter().any(|path| watched.starts_with(watch_path(path)))
                    })).cloned().collect();
                for path in stale { let _ = watcher.unwatch(&path); watches.remove(&path); }
                // Subscribe before discovering, so changes during traversal are queued.
                reconcile_watches(&mut watcher, &mut watches, index.watch_plan());
                let mut batch = ReloadBatch::default();
                let package_change = pending.events.iter().flat_map(|event| &event.paths).any(|path| {
                    let path = watch_path(path);
                    index.roots.packages.iter().any(|root| path.starts_with(root) || root.starts_with(&path))
                });
                if !index.initialized || pending.rescan || package_change {
                    let added = index.set_roots(roots(), &mut batch);
                    reconcile_watches(&mut watcher, &mut watches, index.watch_plan());
                    if index.initialized && !pending.rescan {
                        for root in added {
                            for path in index.scan(&root) { index.custom.insert(path.clone()); batch.custom(path); }
                        }
                    }
                }
                if !index.initialized {
                    index.scan_all();
                    index.initialized = true;
                } else if pending.rescan {
                    batch.paths.extend(index.sources.iter().cloned());
                    let previous = index.custom.clone();
                    index.scan_all();
                    for path in previous.union(&index.custom) { batch.custom(path.clone()); }
                }
                for event in pending.events { index.apply(event, &mut batch); }
                // Missing/recreated roots and source parents may have changed.
                reconcile_watches(&mut watcher, &mut watches, index.watch_plan());
                #[cfg(test)]
                {
                    let mut probe = shared.probe.lock().unwrap();
                    probe.passes += 1;
                    probe.directories_scanned = index.directories_scanned;
                    probe.custom = index.custom.clone();
                    probe.sources = index.sources.clone();
                }
                shared.publish(batch);
            }
        })?;
        Ok(Self { mailbox, thread: Some(thread) })
    }

    pub fn set_sources(&self, sources: Vec<PathBuf>) {
        self.mailbox.pending.lock().unwrap().sources = Some(sources);
        self.mailbox.wake.notify_one();
    }

    pub fn poll(&self) -> ReloadBatch {
        std::mem::take(&mut *self.mailbox.ready.lock().unwrap())
    }
}

impl Drop for DiscoveryWorker {
    fn drop(&mut self) {
        self.mailbox.pending.lock().unwrap().stop = true;
        self.mailbox.wake.notify_one();
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

#[derive(Default)]
pub(super) struct DiscoveryRoots {
    pub custom: BTreeSet<PathBuf>,
    pub packages: BTreeSet<PathBuf>,
}

impl DiscoveryRoots {
    fn normalized(self) -> Self {
        Self {
            custom: self.custom.iter().map(|path| watch_path(path)).collect(),
            packages: self.packages.iter().map(|path| watch_path(path)).collect(),
        }
    }
}

struct DiscoveryIndex {
    roots: DiscoveryRoots,
    sources: BTreeSet<PathBuf>,
    custom: BTreeSet<PathBuf>,
    initialized: bool,
    directories_scanned: usize,
}

impl DiscoveryIndex {
    fn new(roots: DiscoveryRoots) -> Self {
        Self { roots: roots.normalized(), sources: BTreeSet::new(), custom: BTreeSet::new(),
            initialized: false, directories_scanned: 0 }
    }

    fn set_sources(&mut self, sources: Vec<PathBuf>) {
        self.sources = sources.iter().map(|path| watch_path(path)).collect();
    }

    fn set_roots(&mut self, roots: DiscoveryRoots, batch: &mut ReloadBatch) -> Vec<PathBuf> {
        let roots = roots.normalized();
        let added: Vec<_> = roots.custom.difference(&self.roots.custom).cloned().collect();
        self.roots = roots;
        let removed: Vec<_> = self.custom.iter().filter(|path| !self.eligible_location(path)).cloned().collect();
        for path in removed { self.custom.remove(&path); batch.custom(path); }
        added
    }

    fn eligible_location(&self, path: &Path) -> bool {
        self.roots.custom.iter().any(|root| path.strip_prefix(root).is_ok_and(|relative| {
            // ui.lisp must belong to a content subdirectory, as in the generators.
            relative.components().count() >= 2 && !relative.components().any(|part| {
                part.as_os_str().to_str().is_some_and(|name| name.starts_with('.'))
            })
        }))
    }

    fn scan(&mut self, directory: &Path) -> BTreeSet<PathBuf> {
        let mut found = BTreeSet::new();
        let mut pending = vec![directory.to_path_buf()];
        let mut visited = BTreeSet::new();
        while let Some(dir) = pending.pop() {
            if !visited.insert(watch_path(&dir)) { continue; }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue; };
            self.directories_scanned += 1;
            let ui = dir.join("ui.lisp");
            if self.eligible_location(&ui) && ui.is_file() && dir.join("dsp.lisp").is_file() {
                found.insert(ui);
            }
            for entry in entries.flatten() {
                if entry.file_name().to_str().is_some_and(|name| name.starts_with('.')) { continue; }
                if entry.path().is_dir() { pending.push(entry.path()); }
            }
        }
        found
    }

    fn scan_all(&mut self) {
        self.custom.clear();
        for root in self.roots.custom.clone() { let found = self.scan(&root); self.custom.extend(found); }
    }

    fn apply(&mut self, event: Event, batch: &mut ReloadBatch) {
        if matches!(event.kind, EventKind::Access(_)) { return; }
        for path in event.paths {
            let path = watch_path(&path);
            batch.paths.extend(self.sources.iter().filter(|source| source.starts_with(&path)).cloned());
            if matches!(path.file_name().and_then(|name| name.to_str()), Some("ui.lisp" | "dsp.lisp")) {
                let ui = path.with_file_name("ui.lisp");
                if !self.eligible_location(&ui) { continue; }
                let was_present = self.custom.contains(&ui);
                let present = ui.is_file() && ui.with_file_name("dsp.lisp").is_file();
                if present { self.custom.insert(ui.clone()); } else { self.custom.remove(&ui); }
                if was_present != present || (present && path == ui) { batch.custom(ui); }
            } else if self.roots.custom.iter().any(|root| path.starts_with(root) || root.starts_with(&path)) {
                let previous: BTreeSet<_> = self.custom.iter().filter(|ui| ui.starts_with(&path)).cloned().collect();
                let scopes: Vec<_> = if self.roots.custom.iter().any(|root| path.starts_with(root)) {
                    vec![path.clone()]
                } else {
                    self.roots.custom.iter().filter(|root| root.starts_with(&path)).cloned().collect()
                };
                let mut found = BTreeSet::new();
                for scope in scopes {
                    if scope.is_dir() && (self.roots.custom.contains(&scope)
                        || self.eligible_location(&scope.join("ui.lisp")))
                    { found.extend(self.scan(&scope)); }
                }
                for ui in &previous { self.custom.remove(ui); }
                self.custom.extend(found.iter().cloned());
                // Directory events may be coalesced by the OS, including edits
                // under an unchanged directory name. Reconcile its known UIs too.
                for ui in previous.union(&found) { batch.custom(ui.clone()); }
            }
        }
    }

    fn watch_plan(&self) -> BTreeMap<PathBuf, RecursiveMode> {
        let mut plan = BTreeMap::new();
        let mut observe = |path: &Path, recursive: bool| {
            if path.is_dir() && recursive { plan.insert(path.to_path_buf(), RecursiveMode::Recursive); }
            let parent = if recursive { path.parent() } else { Some(path) };
            if let Some(parent) = parent.and_then(existing_ancestor) {
                plan.entry(parent).or_insert(RecursiveMode::NonRecursive);
            }
            // Parent sentinels survive replacing/removing the watched directory.
            if let Some(parent) = path.parent().and_then(existing_ancestor) {
                plan.entry(parent).or_insert(RecursiveMode::NonRecursive);
            }
        };
        for root in self.roots.custom.iter().chain(&self.roots.packages) { observe(root, true); }
        for source in &self.sources { if let Some(parent) = source.parent() { observe(parent, false); } }
        let recursive: Vec<_> = plan.iter().filter(|(_, mode)| **mode == RecursiveMode::Recursive)
            .map(|(path, _)| path.clone()).collect();
        plan.retain(|path, _| !recursive.iter().any(|root| root != path && path.starts_with(root)));
        plan
    }
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|path| path.is_dir()).map(Path::to_path_buf)
}

fn reconcile_watches(watcher: &mut impl Watcher, active: &mut BTreeMap<PathBuf, RecursiveMode>, desired: BTreeMap<PathBuf, RecursiveMode>) {
    let removed: Vec<_> = active.iter().filter(|(path, mode)| desired.get(*path) != Some(*mode))
        .map(|(path, _)| path.clone()).collect();
    for path in removed { let _ = watcher.unwatch(&path); active.remove(&path); }
    for (path, mode) in desired {
        if active.contains_key(&path) { continue; }
        match watcher.watch(&path, mode) {
            Ok(()) => { active.insert(path, mode); }
            Err(error) => eprintln!("metal_seq: Lisp hot reload failed to watch {}: {error}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{DataChange, Flag, ModifyKind, RemoveKind};

    fn roots(root: &Path) -> DiscoveryRoots {
        DiscoveryRoots { custom: [root.to_path_buf()].into_iter().collect(), ..Default::default() }
    }

    fn instrument(directory: &Path) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("dsp.lisp"), "(out 0)").unwrap();
        let ui = directory.join("ui.lisp");
        std::fs::write(&ui, "(defsynth-ui (label 1))").unwrap();
        watch_path(&ui)
    }

    fn edit(path: &Path) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content))).add_path(path.to_path_buf())
    }

    #[test]
    fn incremental_index_handles_ui_dsp_lifecycle_without_rescanning_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let root = watch_path(temp.path());
        let existing = instrument(&root.join("existing"));
        instrument(&root.join(".hidden"));
        let mut index = DiscoveryIndex::new(roots(&root));
        index.scan_all();
        assert_eq!(index.custom, [existing.clone()].into_iter().collect());
        let scanned = index.directories_scanned;
        let new = root.join("new/ui.lisp");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, "(defsynth-ui (label 2))").unwrap();
        let mut batch = ReloadBatch::default();
        index.apply(edit(&new), &mut batch);
        assert!(batch.is_empty(), "UI without DSP is not a generated custom source");
        let dsp = new.with_file_name("dsp.lisp");
        std::fs::write(&dsp, "(out 0)").unwrap();
        index.apply(edit(&dsp), &mut batch);
        assert_eq!(batch.custom_ui, [new.clone()].into_iter().collect());
        batch = ReloadBatch::default();
        index.apply(edit(&dsp), &mut batch);
        assert!(batch.is_empty(), "DSP content edits do not rebuild UI");
        index.apply(edit(&new), &mut batch);
        assert!(batch.custom_ui.contains(&new));
        std::fs::remove_file(&dsp).unwrap();
        batch = ReloadBatch::default();
        index.apply(Event::new(EventKind::Remove(RemoveKind::File)).add_path(dsp), &mut batch);
        assert!(batch.custom_ui.contains(&new), "losing eligibility removes its dispatch");
        assert!(!index.custom.contains(&new));
        assert_eq!(index.directories_scanned, scanned, "file events perform no recursive scans");
        assert!(index.custom.contains(&existing));
    }

    #[test]
    fn moved_directories_and_removed_roots_reconcile_old_and_new_custom_sources() {
        let temp = tempfile::tempdir().unwrap();
        let root = watch_path(temp.path());
        let old = instrument(&root.join("old"));
        let mut index = DiscoveryIndex::new(roots(&root));
        index.scan_all();
        std::fs::rename(root.join("old"), root.join("new")).unwrap();
        let new = root.join("new/ui.lisp");
        let mut batch = ReloadBatch::default();
        index.apply(Event::new(EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Both)))
            .add_path(root.join("old")).add_path(root.join("new")), &mut batch);
        assert_eq!(index.custom, [new.clone()].into_iter().collect());
        assert_eq!(batch.custom_ui, [old, new.clone()].into_iter().collect());
        batch = ReloadBatch::default();
        index.set_roots(DiscoveryRoots::default(), &mut batch);
        assert!(index.custom.is_empty());
        assert!(batch.custom_ui.contains(&new));
    }

    #[test]
    fn mailbox_bounds_event_storms_and_preserves_os_rescan_requests() {
        let mailbox = Mailbox::default();
        for _ in 0..2048 { mailbox.event(Ok(edit(Path::new("/tmp/ui.lisp")))); }
        let pending = mailbox.pending.lock().unwrap();
        assert!(pending.rescan);
        assert!(pending.events.is_empty());
        drop(pending);
        let mailbox = Mailbox::default();
        mailbox.event(Ok(Event::new(EventKind::Access(notify::event::AccessKind::Any)).set_flag(Flag::Rescan)));
        assert!(mailbox.pending.lock().unwrap().rescan, "even access events can carry loss flags");
        mailbox.event(Err(notify::Error::generic("lost stream")));
        assert!(mailbox.pending.lock().unwrap().rescan);
        mailbox.pending.lock().unwrap().sources = Some(vec![PathBuf::from("/tmp/new.lisp")]);
        assert!(!mailbox.next().rescan, "source registration does not bypass event debouncing");
        assert!(mailbox.pending.lock().unwrap().rescan);
    }

    fn wait_for(worker: &DiscoveryWorker, label: &str, mut predicate: impl FnMut(&WorkerProbe, &ReloadBatch) -> bool) -> ReloadBatch {
        let started = Instant::now();
        let mut all = ReloadBatch::default();
        loop {
            let batch = worker.poll();
            all.paths.extend(batch.paths);
            all.custom_ui.extend(batch.custom_ui);
            let probe = worker.mailbox.probe.lock().unwrap().clone();
            if predicate(&probe, &all) { return all; }
            assert!(started.elapsed() < Duration::from_secs(12), "native watcher timed out: {label}; changes={all:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn native_watcher_discovers_atomic_saves_and_recreated_roots() {
        let temp = tempfile::tempdir().unwrap();
        let parent = watch_path(temp.path());
        let root = parent.join("instruments");
        let root_for_worker = root.clone();
        let worker = DiscoveryWorker::start(Vec::new(), move || roots(&root_for_worker)).unwrap();
        wait_for(&worker, "initial missing root", |probe, _| probe.passes > 0);
        let ui = instrument(&root.join("nested/synth"));
        wait_for(&worker, "new root and nested instrument", |probe, batch| probe.custom.contains(&ui) && batch.custom_ui.contains(&ui));

        let atomic = ui.with_file_name(".ui-save");
        std::fs::write(&atomic, "(defsynth-ui (label 2))").unwrap();
        std::fs::rename(&atomic, &ui).unwrap();
        wait_for(&worker, "atomic save", |_, batch| batch.custom_ui.contains(&ui));

        std::fs::rename(&root, parent.join("removed")).unwrap();
        wait_for(&worker, "root moved away", |probe, batch| probe.custom.is_empty() && batch.custom_ui.contains(&ui));
        let replacement = instrument(&root.join("replacement"));
        wait_for(&worker, "recreated root", |probe, batch| probe.custom.contains(&replacement) && batch.custom_ui.contains(&replacement));
        std::fs::write(&replacement, "(defsynth-ui (label 3))").unwrap();
        wait_for(&worker, "replacement still watched", |_, batch| batch.custom_ui.contains(&replacement));

        // Exceptional rescan follows the same worker path as native OS loss.
        worker.mailbox.event(Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
        wait_for(&worker, "loss reconciliation", |_, batch| batch.custom_ui.contains(&replacement));
    }

    #[test]
    fn native_watcher_tracks_source_revisions_and_recovers_missing_file_parents() {
        let temp = tempfile::tempdir().unwrap();
        let root = watch_path(temp.path());
        let init = root.join("config/init.lisp");
        let worker = DiscoveryWorker::start(vec![init.clone()], DiscoveryRoots::default).unwrap();
        wait_for(&worker, "source registration", |probe, _| probe.sources.contains(&init));
        std::fs::create_dir_all(init.parent().unwrap()).unwrap();
        std::fs::write(&init, "(def init 1)").unwrap();
        wait_for(&worker, "missing init created", |_, batch| batch.paths.contains(&init));
        let source = root.join("loaded/child.lisp");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "(def child 1)").unwrap();
        worker.set_sources(vec![source.clone()]);
        wait_for(&worker, "new module graph", |probe, _| probe.sources.contains(&source) && !probe.sources.contains(&init));
        std::fs::write(&source, "(def child 2)").unwrap();
        wait_for(&worker, "loaded source edit", |_, batch| batch.paths.contains(&source));
        std::fs::remove_dir_all(source.parent().unwrap()).unwrap();
        wait_for(&worker, "loaded parent deletion", |_, batch| batch.paths.contains(&source));
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "(def child 3)").unwrap();
        wait_for(&worker, "loaded parent restored", |_, batch| batch.paths.contains(&source));
    }

    #[test]
    fn native_watcher_discovers_installed_package_roots_and_manifest_removal() {
        let temp = tempfile::tempdir().unwrap();
        let root = watch_path(temp.path());
        let paths = sequencer::app_paths::AppPaths::dev(root.join("workspace/crates/sequencer"), root.join("workspace"), root.join("config"));
        paths.ensure_user_tier().unwrap();
        let packages = paths.packages_dir();
        let worker = DiscoveryWorker::start(Vec::new(), move || DiscoveryRoots {
            custom: paths.instrument_dirs().into_iter().chain(paths.effect_dirs()).collect(),
            packages: [paths.packages_dir(), paths.factory_packages_dir()].into_iter().collect(),
        }).unwrap();
        wait_for(&worker, "package subscriptions", |probe, _| probe.passes > 0);
        let package = packages.join("dev.probe");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(package.join("src/main.lisp"), "(module dev.probe.main)").unwrap();
        let ui = instrument(&package.join("instruments/probe"));
        let manifest = package.join("manifest.json");
        std::fs::write(&manifest, r#"{"name":"dev/probe","version":"1","entry":"dev.probe.main"}"#).unwrap();
        wait_for(&worker, "installed package UI", |probe, batch| probe.custom.contains(&ui) && batch.custom_ui.contains(&ui));
        std::fs::remove_file(&manifest).unwrap();
        wait_for(&worker, "removed manifest", |probe, batch| !probe.custom.contains(&ui) && batch.custom_ui.contains(&ui));
    }

    #[test]
    fn idle_ui_polls_do_not_scan_or_wake_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let root = watch_path(temp.path());
        for index in 0..64 { instrument(&root.join(format!("synth-{index}"))); }
        let worker = DiscoveryWorker::start(Vec::new(), move || roots(&root)).unwrap();
        wait_for(&worker, "initial index", |probe, _| probe.custom.len() == 64);
        // Let any initial native subscription events settle before measuring idle.
        std::thread::sleep(Duration::from_millis(400));
        let before = worker.mailbox.probe.lock().unwrap().clone();
        let started = Instant::now();
        for _ in 0..100_000 { assert!(worker.poll().is_empty()); }
        let poll_time = started.elapsed();
        std::thread::sleep(Duration::from_millis(1200));
        let after = worker.mailbox.probe.lock().unwrap().clone();
        assert_eq!(after.directories_scanned, before.directories_scanned);
        assert_eq!(after.passes, before.passes, "worker sleeps until a notification");
        eprintln!("100000 idle UI polls: {poll_time:?}; additional directory scans: 0; worker passes: 0");
    }
}
