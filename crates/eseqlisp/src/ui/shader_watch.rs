//! Native change notifications for a development shader override. Idle render
//! ticks only inspect an atomic flag; file reads happen at startup or on an event.

use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub(super) struct ShaderFileWatch {
    path: PathBuf,
    changed: Arc<AtomicBool>,
    last_source: Option<String>,
    // Dropping the owner unregisters the watch and stops its notification worker.
    _watcher: Option<RecommendedWatcher>,
}

fn affects_shader(event: &Event, path: &Path, directory: &Path) -> bool {
    event.need_rescan() || (!matches!(event.kind, EventKind::Access(_))
        && (event.paths.is_empty() || event.paths.iter().any(|p| p == path || p == directory)))
}

impl ShaderFileWatch {
    pub(super) fn new(path: PathBuf) -> Self {
        // Watch the directory, not the file inode: editors often save by
        // replacing the file, and a temporarily missing file must be recoverable.
        let directory = path.parent().expect("shader override has a parent directory");
        let path = directory.canonicalize().ok()
            .and_then(|directory| path.file_name().map(|name| directory.join(name)))
            .unwrap_or(path);
        let directory = path.parent().expect("shader override has a parent directory").to_path_buf();
        let changed = Arc::new(AtomicBool::new(true));
        let pending = changed.clone();
        let watched_path = path.clone();
        let watched_directory = directory.clone();
        let watcher = RecommendedWatcher::new(move |result: notify::Result<Event>| {
            match result {
                Ok(event) if affects_shader(&event, &watched_path, &watched_directory) => {
                    pending.store(true, Ordering::Release);
                }
                Ok(_) => {},
                Err(error) => {
                    eprintln!("[button-shader-watch] notification error: {error}");
                    // A lost-event/error notification warrants one reconciliation.
                    pending.store(true, Ordering::Release);
                }
            }
        }, Config::default()).and_then(|mut watcher| {
            watcher.watch(&directory, RecursiveMode::NonRecursive)?;
            Ok(watcher)
        });
        let watcher = match watcher {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                eprintln!("[button-shader-watch] automatic reload unavailable for {}: {error}", path.display());
                None
            }
        };
        Self { path, changed, last_source: None, _watcher: watcher }
    }

    pub(super) fn take_changed_source(&mut self) -> Option<String> {
        // Clear before reading so an edit arriving during the read remains
        // pending. Duplicate/coalesced notifications are checked by contents,
        // avoiding both redundant compilations and mtime-resolution races.
        if !self.changed.swap(false, Ordering::AcqRel) {
            return None;
        }
        let source = match std::fs::read_to_string(&self.path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("[button-shader-watch] could not read {}: {error}", self.path.display());
                return None;
            }
        };
        if self.last_source.as_ref() == Some(&source) {
            return None;
        }
        // Failed compilations are also attempted just once per source version;
        // the renderer retains its last valid pipelines until a later edit works.
        self.last_source = Some(source.clone());
        Some(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use notify::event::{AccessKind, DataChange, Flag, ModifyKind, RenameMode};

    #[test]
    fn shader_events_include_replacement_and_rescan_but_not_reads_or_other_files() {
        let directory = Path::new("/shaders");
        let path = directory.join("button.metal");
        assert!(affects_shader(&Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(directory.join("temporary")).add_path(path.clone()), &path, directory));
        assert!(affects_shader(&Event::new(EventKind::Other).set_flag(Flag::Rescan), &path, directory));
        assert!(!affects_shader(&Event::new(EventKind::Access(AccessKind::Any))
            .add_path(path.clone()), &path, directory));
        assert!(!affects_shader(&Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(directory.join("other.metal")), &path, directory));
    }

    #[test]
    fn native_shader_watch_handles_edit_atomic_replace_and_delete_recreate() {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let directory = Directory(std::env::temp_dir().join(format!("eseq-shader-watch-{}-{}",
            std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::create_dir_all(&directory.0).unwrap();
        let path = directory.0.join("button.metal");
        std::fs::write(&path, "initial").unwrap();
        let mut watch = ShaderFileWatch::new(path.clone());
        assert!(watch._watcher.is_some(), "native file notifications must be available");
        assert_eq!(watch.take_changed_source().as_deref(), Some("initial"));
        assert!(watch.take_changed_source().is_none());

        fn await_source(watch: &mut ShaderFileWatch, expected: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if watch.take_changed_source().as_deref() == Some(expected) { return; }
                assert!(Instant::now() < deadline, "missing native shader notification for {expected}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        std::fs::write(&path, "edited").unwrap();
        await_source(&mut watch, "edited");
        let replacement = directory.0.join("temporary");
        std::fs::write(&replacement, "replaced").unwrap();
        std::fs::rename(replacement, &path).unwrap();
        await_source(&mut watch, "replaced");

        std::fs::remove_file(&path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !watch.changed.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "missing deletion notification");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(watch.take_changed_source().is_none());
        std::fs::write(&path, "restored").unwrap();
        await_source(&mut watch, "restored");
        // Touching/re-saving identical contents must not cause a recompilation.
        watch.changed.store(true, Ordering::Release);
        assert!(watch.take_changed_source().is_none());
    }
}
