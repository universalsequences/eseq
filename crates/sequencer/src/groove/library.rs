//! The groove library (docs/rack-groove-spec.md §Three tiers, bead
//! eseq-groove.9): versioned `.groove` files in a read-only factory tier
//! (`AppPaths::grooves_dir`, bundle `content/grooves/`) and a mutable user
//! tier (`AppPaths::user_grooves_dir`), listed merged like kits
//! (`project::list_kit_presets`: factory first, then user, each sorted).
//!
//! The library is never played directly: applying a library groove copies
//! it into the project pool first (`pool::import_groove`), so a project plays
//! the same on a machine without the file and editing the library never
//! changes an existing project. Saving a pool groove to the library is the
//! reverse copy; it never links.
//!
//! Every IO function has a `*_in` core that takes its directories, so tests
//! run against temp dirs; the plain names resolve the app's tiers.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::{GrooveChoice, GroovePadRow, GrooveRow, ProjectGroove};

/// The `.groove` payload generation this build writes. 1 = the rev-2 file:
/// a `ProjectGroove` without `id`, plus this version.
pub const GROOVE_FILE_VERSION: u32 = 1;
pub const GROOVE_FILE_EXTENSION: &str = "groove";

/// One `.groove` file: a [`ProjectGroove`] without `id` (ids are pool-local).
/// Named by the file stem unless `name` is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrooveFile {
    /// Absent reads as 1 (the first generation).
    #[serde(default = "first_groove_file_version")]
    pub groove_version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub period_beats: f64,
    pub resolution_beats: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pad_rows: Vec<GroovePadRow>,
    pub shared_row: GrooveRow,
}

fn first_groove_file_version() -> u32 {
    1
}

impl GrooveFile {
    pub fn from_groove(groove: &ProjectGroove) -> Self {
        Self {
            groove_version: GROOVE_FILE_VERSION,
            name: groove.name.clone(),
            period_beats: groove.period_beats,
            resolution_beats: groove.resolution_beats,
            pad_rows: groove.pad_rows.clone(),
            shared_row: groove.shared_row.clone(),
        }
    }

    /// The groove this file holds (id 0: it has no pool id until imported),
    /// named `stem` when the file sets no name.
    pub fn into_groove(self, stem: &str) -> ProjectGroove {
        let name = if self.name.trim().is_empty() {
            stem.to_string()
        } else {
            self.name.trim().to_string()
        };
        ProjectGroove {
            id: 0,
            name,
            period_beats: self.period_beats,
            resolution_beats: self.resolution_beats,
            pad_rows: self.pad_rows,
            shared_row: self.shared_row,
        }
    }
}

/// Which library tier a file lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GrooveLibraryTier {
    /// Shipped in the bundle; read-only.
    Factory,
    /// The user's own saves; rename/delete allowed.
    User,
}

impl GrooveLibraryTier {
    /// The tier's picker-key prefix: `factory` or `user`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Factory => "factory",
            Self::User => "user",
        }
    }
}

/// One listed library groove.
#[derive(Clone, Debug, PartialEq)]
pub struct GrooveLibraryEntry {
    pub tier: GrooveLibraryTier,
    pub path: PathBuf,
    pub stem: String,
    /// Display name: the file's `name`, else its stem.
    pub name: String,
}

impl GrooveLibraryEntry {
    pub fn choice(&self) -> GrooveChoice {
        GrooveChoice::Library {
            tier: self.tier,
            stem: self.stem.clone(),
        }
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A stem a picker key may carry: non-empty, no path separators or dots
/// that could walk out of the tier directory.
pub(super) fn is_plain_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem != "."
        && stem != ".."
        && !stem.contains(['/', '\\'])
        && !stem.starts_with('.')
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Groove")
        .to_string()
}

/// Reads and validates one `.groove` file: a newer generation or a malformed
/// grid is an error rather than a groove the scheduler would trust.
pub fn read_groove_file(path: &Path) -> io::Result<ProjectGroove> {
    let src = std::fs::read_to_string(path)?;
    let file: GrooveFile = serde_json::from_str(&src).map_err(|error| {
        invalid(format!(
            "Failed to parse groove '{}': {error}",
            path.display()
        ))
    })?;
    if file.groove_version > GROOVE_FILE_VERSION {
        return Err(invalid(format!(
            "Groove '{}' is version {}; this build reads up to {GROOVE_FILE_VERSION}",
            path.display(),
            file.groove_version
        )));
    }
    let groove = file.into_groove(&file_stem(path));
    if !groove.is_well_formed() {
        return Err(invalid(format!("Groove '{}' is malformed", path.display())));
    }
    Ok(groove)
}

/// Writes `groove` as a current-generation `.groove` file.
pub fn write_groove_file(path: &Path, groove: &ProjectGroove) -> io::Result<()> {
    if !groove.is_well_formed() {
        return Err(invalid(format!("Groove '{}' is malformed", groove.name)));
    }
    let json = serde_json::to_string_pretty(&GrooveFile::from_groove(groove))
        .map_err(|error| io::Error::other(format!("Failed to serialize groove: {error}")))?;
    std::fs::write(path, json)
}

fn list_tier(dir: &Path, tier: GrooveLibraryTier, out: &mut Vec<GrooveLibraryEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some(GROOVE_FILE_EXTENSION))
        .collect::<Vec<_>>();
    // By stem, not path: `a-2.groove` would otherwise sort before `a.groove`.
    paths.sort_by_key(|path| file_stem(path));
    for path in paths {
        // An unreadable or malformed file is not offered: picking it could
        // only fail.
        let Ok(groove) = read_groove_file(&path) else {
            continue;
        };
        out.push(GrooveLibraryEntry {
            tier,
            stem: file_stem(&path),
            name: groove.name,
            path,
        });
    }
}

/// The merged library: every valid factory file (sorted by stem), then every
/// valid user file. A missing directory is simply empty.
pub fn list_groove_library_in(factory_dir: &Path, user_dir: &Path) -> Vec<GrooveLibraryEntry> {
    let mut entries = Vec::new();
    list_tier(factory_dir, GrooveLibraryTier::Factory, &mut entries);
    list_tier(user_dir, GrooveLibraryTier::User, &mut entries);
    entries
}

fn tier_dir<'a>(tier: GrooveLibraryTier, factory_dir: &'a Path, user_dir: &'a Path) -> &'a Path {
    match tier {
        GrooveLibraryTier::Factory => factory_dir,
        GrooveLibraryTier::User => user_dir,
    }
}

fn file_in(dir: &Path, stem: &str) -> io::Result<PathBuf> {
    if !is_plain_stem(stem) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Bad groove file name {stem:?}"),
        ));
    }
    Ok(dir.join(format!("{stem}.{GROOVE_FILE_EXTENSION}")))
}

/// Loads one library groove by tier and stem.
pub fn load_library_groove_in(
    factory_dir: &Path,
    user_dir: &Path,
    tier: GrooveLibraryTier,
    stem: &str,
) -> io::Result<ProjectGroove> {
    read_groove_file(&file_in(tier_dir(tier, factory_dir, user_dir), stem)?)
}

/// A file stem for `name` in `dir` that no existing file uses: the
/// sanitized name, then `-2`, `-3`, ... Library edits are not undoable, so a
/// save never overwrites.
fn free_stem(dir: &Path, name: &str) -> io::Result<String> {
    let base = crate::project::sanitize_project_name(name);
    let base = if base.is_empty() {
        "groove".to_string()
    } else {
        base
    };
    let mut stem = base.clone();
    let mut n = 2;
    while file_in(dir, &stem)?.exists() {
        stem = format!("{base}-{n}");
        n += 1;
    }
    Ok(stem)
}

/// "Save to Library": writes a copy of `groove` named `name` into the user
/// tier and returns the new file. Never overwrites an existing file.
pub fn save_groove_to_library_in(
    user_dir: &Path,
    name: &str,
    groove: &ProjectGroove,
) -> io::Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Groove name cannot be empty",
        ));
    }
    std::fs::create_dir_all(user_dir)?;
    let path = file_in(user_dir, &free_stem(user_dir, name)?)?;
    let mut copy = groove.clone();
    copy.name = name.to_string();
    write_groove_file(&path, &copy)?;
    Ok(path)
}

/// Renames one user-tier groove: the file moves to the new name's stem and
/// its `name` is rewritten. Returns the new path.
pub fn rename_library_groove_in(user_dir: &Path, stem: &str, name: &str) -> io::Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Groove name cannot be empty",
        ));
    }
    let old = file_in(user_dir, stem)?;
    let mut groove = read_groove_file(&old)?;
    groove.name = name.to_string();
    let wanted = crate::project::sanitize_project_name(name);
    let new = if wanted == stem {
        old.clone()
    } else {
        file_in(user_dir, &free_stem(user_dir, name)?)?
    };
    write_groove_file(&new, &groove)?;
    if new != old {
        std::fs::remove_file(&old)?;
    }
    Ok(new)
}

/// Deletes one user-tier groove file.
pub fn delete_library_groove_in(user_dir: &Path, stem: &str) -> io::Result<()> {
    std::fs::remove_file(file_in(user_dir, stem)?)
}

// --- the app's tiers --------------------------------------------------------

/// Test-only redirect of the app's (factory, user) groove directories, so a
/// host-command test saves, renames and deletes library files in a temp dir
/// instead of the developer's `.local/grooves`. Process-wide: nextest runs
/// each test in its own process.
static TEST_LIBRARY_DIRS: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

#[doc(hidden)]
pub fn override_groove_library_dirs_for_tests(dirs: Option<(PathBuf, PathBuf)>) {
    if let Ok(mut slot) = TEST_LIBRARY_DIRS.lock() {
        *slot = dirs;
    }
    invalidate_library_listing();
}

fn app_dirs() -> (PathBuf, PathBuf) {
    if let Some(dirs) = TEST_LIBRARY_DIRS.lock().ok().and_then(|slot| slot.clone()) {
        return dirs;
    }
    let paths = crate::app_paths::app_paths();
    (paths.grooves_dir(), paths.user_grooves_dir())
}

/// The last app-tier listing and what it was read from. `SEQ` groove state
/// is republished on every undo/redo, groove command and groups change, so
/// re-reading and parsing every `.groove` file each time would put a dozen
/// file reads on the main thread per sync. Reuse the listing while both tier
/// directories and their mtimes are unchanged (adding, removing or renaming a
/// file bumps its directory's mtime); the app's own save/rename/delete also
/// drop it explicitly, since an in-place rewrite leaves the mtime alone.
struct LibraryListingCache {
    factory: PathBuf,
    user: PathBuf,
    stamps: (Option<SystemTime>, Option<SystemTime>),
    entries: Vec<GrooveLibraryEntry>,
}

static LIBRARY_LISTING: Mutex<Option<LibraryListingCache>> = Mutex::new(None);

fn dir_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|meta| meta.modified()).ok()
}

/// Moved by every [`invalidate_library_listing`] (an app save, rename or
/// delete, or a test's directory redirect).
static LIBRARY_GENERATION: AtomicU64 = AtomicU64::new(0);

/// A counter moved whenever the app's own edits drop the cached listing, so
/// a reader can skip re-listing while it holds still (an external edit is
/// caught by the listing's directory mtimes instead).
pub fn library_generation() -> u64 {
    LIBRARY_GENERATION.load(Ordering::Relaxed)
}

fn invalidate_library_listing() {
    if let Ok(mut cache) = LIBRARY_LISTING.lock() {
        *cache = None;
    }
    LIBRARY_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The app's merged library, cached (see `LibraryListingCache`).
pub fn list_groove_library() -> Vec<GrooveLibraryEntry> {
    let (factory, user) = app_dirs();
    let stamps = (dir_mtime(&factory), dir_mtime(&user));
    let Ok(mut cache) = LIBRARY_LISTING.lock() else {
        return list_groove_library_in(&factory, &user);
    };
    if let Some(hit) = cache
        .as_ref()
        .filter(|hit| hit.factory == factory && hit.user == user && hit.stamps == stamps)
    {
        return hit.entries.clone();
    }
    let entries = list_groove_library_in(&factory, &user);
    *cache = Some(LibraryListingCache {
        factory,
        user,
        stamps,
        entries: entries.clone(),
    });
    entries
}

pub fn load_library_groove(tier: GrooveLibraryTier, stem: &str) -> io::Result<ProjectGroove> {
    let (factory, user) = app_dirs();
    load_library_groove_in(&factory, &user, tier, stem)
}

pub fn save_groove_to_library(name: &str, groove: &ProjectGroove) -> io::Result<PathBuf> {
    let saved = save_groove_to_library_in(&app_dirs().1, name, groove);
    invalidate_library_listing();
    saved
}

pub fn rename_library_groove(stem: &str, name: &str) -> io::Result<PathBuf> {
    let renamed = rename_library_groove_in(&app_dirs().1, stem, name);
    invalidate_library_listing();
    renamed
}

pub fn delete_library_groove(stem: &str) -> io::Result<()> {
    let deleted = delete_library_groove_in(&app_dirs().1, stem);
    invalidate_library_listing();
    deleted
}
