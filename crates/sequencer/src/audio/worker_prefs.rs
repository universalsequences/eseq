//! How many audiograph helper threads the engine starts (eseq-6jr2).
//!
//! A saved preference, `audio.json` beside `midi-input.json`, read once when
//! the engine starts. Changing it takes effect next launch: helpers are bound
//! to the device workgroup (`workgroup.rs`) and are never respawned under a
//! running stream. `TINYSEQ_AUDIOGRAPH_WORKERS` still overrides the saved
//! value, so benchmarking needs no settings round trip.
//!
//! Unset means Auto: the machine's performance cores minus two (one for the
//! audio callback, one for the UI), clamped to [`AUTO_MIN`]..=[`AUTO_MAX`].
//! Helpers spin between blocks, so more than the graph can use costs CPU
//! without lowering callback load (docs/audio-idle-wait-fix-2026-09-11.md).
//! Where the performance-core count is unknown (Linux, Intel Macs) Auto keeps
//! the historical fixed default.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// Overrides the saved preference for this launch.
pub const ENV_OVERRIDE: &str = "TINYSEQ_AUDIOGRAPH_WORKERS";
/// Auto never goes below this on a machine that reports its core layout.
pub const AUTO_MIN: u32 = 2;
/// Auto never goes above this, however many cores the machine has.
pub const AUTO_MAX: u32 = 12;
/// Auto where the performance-core count is unknown.
pub const FALLBACK_WORKERS: u32 = 4;
/// Largest explicit choice accepted from disk or the UI.
pub const MAX_WORKERS: u32 = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPrefs {
    /// Explicit helper count; `None` is Auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<u32>,
}

pub fn prefs_path() -> PathBuf {
    crate::app_paths::app_paths()
        .preferences_path()
        .with_file_name("audio.json")
}

pub fn load_from(path: &Path) -> Result<WorkerPrefs, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(WorkerPrefs::default()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn save_to(path: &Path, prefs: WorkerPrefs) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("audio preferences have no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&prefs).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// The saved preference; an unreadable file degrades to Auto.
pub fn load() -> WorkerPrefs {
    let path = prefs_path();
    load_from(&path).unwrap_or_else(|error| {
        eprintln!("[audio-prefs] ignoring {}: {error}", path.display());
        WorkerPrefs::default()
    })
}

pub fn save(prefs: WorkerPrefs) -> Result<(), String> {
    save_to(&prefs_path(), prefs)
}

pub fn auto_worker_count_from(performance_cores: Option<u32>) -> u32 {
    match performance_cores {
        Some(cores) => cores.saturating_sub(2).clamp(AUTO_MIN, AUTO_MAX),
        None => FALLBACK_WORKERS,
    }
}

pub fn auto_worker_count() -> u32 {
    auto_worker_count_from(performance_cores())
}

pub fn resolve(prefs: WorkerPrefs, auto: u32) -> u32 {
    prefs.workers.map_or(auto, |n| n.min(MAX_WORKERS))
}

/// What the next engine start will use, before the environment override.
pub fn startup_worker_count() -> u32 {
    resolve(load(), auto_worker_count())
}

/// The environment override, when set to a valid count.
pub fn env_override() -> Option<u32> {
    let value = std::env::var(ENV_OVERRIDE).ok()?;
    value.trim().parse::<i32>().ok().map(|n| n.max(0) as u32)
}

/// Logical cores, the upper bound the settings picker offers.
pub fn logical_cores() -> u32 {
    std::thread::available_parallelism().map_or(FALLBACK_WORKERS, |n| n.get() as u32)
}

#[cfg(target_os = "macos")]
fn performance_cores() -> Option<u32> {
    // Apple Silicon only; Intel Macs have no perflevels and fall back.
    let name = c"hw.perflevel0.physicalcpu";
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && value > 0).then_some(value as u32)
}

#[cfg(not(target_os = "macos"))]
fn performance_cores() -> Option<u32> {
    None
}

const NOT_STARTED: u32 = u32::MAX;
static RUNNING: AtomicU32 = AtomicU32::new(NOT_STARTED);

pub(super) fn record_running(workers: u32) {
    RUNNING.store(workers, Ordering::Relaxed);
}

/// Helpers the engine started this launch, once it has started.
pub fn running_worker_count() -> Option<u32> {
    match RUNNING.load(Ordering::Relaxed) {
        NOT_STARTED => None,
        n => Some(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_leaves_two_performance_cores_free_within_bounds() {
        assert_eq!(auto_worker_count_from(Some(8)), 6); // M1 Max
        assert_eq!(auto_worker_count_from(Some(4)), AUTO_MIN);
        assert_eq!(auto_worker_count_from(Some(1)), AUTO_MIN);
        assert_eq!(auto_worker_count_from(Some(24)), AUTO_MAX);
        assert_eq!(auto_worker_count_from(None), FALLBACK_WORKERS);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn apple_silicon_reports_its_performance_cores() {
        let cores = performance_cores().expect("hw.perflevel0.physicalcpu");
        assert!(cores <= logical_cores());
        assert_eq!(auto_worker_count(), auto_worker_count_from(Some(cores)));
    }

    #[test]
    fn explicit_choice_beats_auto_and_is_capped() {
        assert_eq!(resolve(WorkerPrefs::default(), 6), 6);
        assert_eq!(resolve(WorkerPrefs { workers: Some(3) }, 6), 3);
        assert_eq!(resolve(WorkerPrefs { workers: Some(0) }, 6), 0);
        assert_eq!(resolve(WorkerPrefs { workers: Some(999) }, 6), MAX_WORKERS);
    }

    #[test]
    fn prefs_round_trip_and_missing_file_is_auto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("audio.json");
        assert_eq!(load_from(&path).unwrap(), WorkerPrefs::default());

        save_to(&path, WorkerPrefs { workers: Some(7) }).unwrap();
        assert_eq!(load_from(&path).unwrap(), WorkerPrefs { workers: Some(7) });

        // Auto serializes as an empty object, not a null field.
        save_to(&path, WorkerPrefs::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "{}");
        assert_eq!(load_from(&path).unwrap(), WorkerPrefs::default());
    }

    #[test]
    fn malformed_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.json");
        std::fs::write(&path, "{\"workers\": \"lots\"}").unwrap();
        assert!(load_from(&path).is_err());
    }
}
