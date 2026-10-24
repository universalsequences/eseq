/*!
On-disk instrument storage (sources, metadata, presets) and the global
registry of loaded instrument process functions.

The storage half resolves instrument names to source/metadata/preset paths
(supporting both flat files and `name/dsp.lisp` folder layouts) and
loads/saves `InstrumentPreset` banks and `CustomInstrumentRunMode` metadata.
The registry half is a set of lock-free static tables indexed by
engine/voice slot (`DGEN_INSTRUMENT_FNS`, output counts, enabled-voice
counts, process-call stats) that the audio thread reads through
`dgenlisp_instrument_vtable()` while the UI/compile side swaps entries in
(`set_dgen_instrument_fn`, ...).
*/

use super::super::*;
use crate::sequencer::MAX_INSTRUMENT_ENGINES;
use crate::audio::MAX_VOICES;
use std::sync::atomic::{AtomicU32, AtomicU8};

// Voice masks hold one bit per engine voice.
const _: () = assert!(MAX_VOICES <= u32::BITS as usize);

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentPreset {
    pub id: String,
    pub name: String,
    pub base_note_offset: f32,
    pub params: std::collections::BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub key_locks: std::collections::BTreeMap<u8, std::collections::BTreeMap<String, f32>>,
}

#[derive(Serialize, Deserialize)]
pub(in crate::lisp_host) struct InstrumentMetadataFile {
    version: u32,
    run_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    voice_controls: Option<InstrumentVoiceControlNames>,
    /// Release manifest (instrument-versioning spec §Manifest). Carried here
    /// so rewriting the run mode keeps it; resolution reads it through
    /// `InstrumentReleaseManifest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    releases: Option<std::collections::BTreeMap<u32, InstrumentReleaseEntry>>,
    /// Deprecation (instrument-versioning spec §Deprecation): hidden from
    /// listings and the browser, still fully loadable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deprecated: bool,
    /// Informational successor logical path, e.g. `Drums/VILLAIN Kick`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replaced_by: Option<String>,
}

/// The deprecation-only view of `instrument.json` listings read.
#[derive(Deserialize)]
struct InstrumentDeprecationManifest {
    #[serde(default)]
    deprecated: bool,
}

/// Whether the `instrument.json` at `metadata_path` marks its instrument
/// deprecated. A missing or unreadable file is not deprecated: hiding an
/// instrument because its metadata is broken would lose it silently.
pub fn instrument_metadata_is_deprecated(metadata_path: &Path) -> bool {
    std::fs::read_to_string(metadata_path)
        .ok()
        .and_then(|source| serde_json::from_str::<InstrumentDeprecationManifest>(&source).ok())
        .is_some_and(|manifest| manifest.deprecated)
}

/// Whether the instrument whose source lives at `source` (a folder's
/// `dsp.lisp` or a single-file `.lisp`) is deprecated.
pub fn instrument_source_is_deprecated(source: &Path) -> bool {
    instrument_metadata_path_for_source_path(source)
        .is_ok_and(|path| instrument_metadata_is_deprecated(&path))
}

/// The release-only view of `instrument.json` the release index scans.
#[derive(Deserialize)]
struct InstrumentReleaseManifest {
    #[serde(default)]
    current: Option<u32>,
    #[serde(default)]
    releases: std::collections::BTreeMap<u32, InstrumentReleaseEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct InstrumentReleaseEntry {
    /// Release folder, relative to the instrument's top folder (`.` or
    /// `versions/<n>`).
    path: String,
    /// The logical path the release shipped under.
    name: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentVoiceControlNames {
    mode: String,
    /// Retired: the track's Voices and Trigger are the only note limit and
    /// legato control. Still accepted so older instrument.json files load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legato: Option<String>,
}

/// Host allocation metadata is independent of the compiler's envelope-only
/// `@role` vocabulary. Apply it after acquiring the DSP artifact, so editing
/// instrument.json cannot leave an old policy in the dylib cache.
pub(in crate::lisp_host) fn apply_instrument_voice_metadata(
    manifest: &mut DGenManifest,
    asset_base: Option<&Path>,
) -> Result<(), String> {
    let Some(base) = asset_base else { return Ok(()); };
    let path = base.join("instrument.json");
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read '{}': {error}", path.display())),
    };
    let metadata: InstrumentMetadataFile = serde_json::from_str(&source)
        .map_err(|error| format!("invalid instrument metadata '{}': {error}", path.display()))?;
    let Some(controls) = metadata.voice_controls else { return Ok(()); };
    let (role, name, min, max) = ("voice-mode", &controls.mode, 0.0, 3.0);
    let matches: Vec<_> = manifest.params.iter().enumerate()
        .filter(|(_, p)| p.name == *name || p.display_name == *name).collect();
    let [(index, param)] = matches.as_slice() else {
        return Err(format!("voice_controls.{role} requires one visible scalar parameter '{name}'"));
    };
    if param.hidden || param.cell_span != 1 || param.min != min || param.max != max || param.role.is_some() {
        return Err(format!("voice_controls.{role} parameter '{name}' must be visible, scalar, unassigned, with range {min}..{max}"));
    }
    let index = *index;
    manifest.params[index].role = Some(role.to_string());
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub(in crate::lisp_host) struct InstrumentPresetBank {
    version: u32,
    engine_name: String,
    source_file: String,
    presets: Vec<InstrumentPreset>,
}

/// Memo for the recursive-walk fallback in `resolve_instrument_storage_path`.
/// Names that don't resolve via the cheap exact-path probes trigger a full
/// scan of the AppPaths instrument roots; hot callers (the glyph feeds re-read sources
/// every reactive tick) must not pay that walk repeatedly. Hits are
/// revalidated with `exists()`, so deleting/moving a source re-resolves on
/// the next call. Known staleness: adding a SECOND source with the same leaf
/// name mid-session won't surface the ambiguity error until the cached path
/// goes away.
fn resolved_walk_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<(String, String), PathBuf>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), PathBuf>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

use crate::app_paths::ContentTier as InstrumentTier;

/// One directory instrument sources resolve from.
struct InstrumentRoot {
    /// The id prefix sources under this root qualify as.
    tier: InstrumentTier,
    path: PathBuf,
    /// Fallback roots resolve only after every primary root has missed, so a
    /// checkout's own factory and user tiers always win over them.
    fallback: bool,
}

/// Instrument roots in resolution order: the factory tier, the user tier,
/// every installed package that ships an `instruments/` directory, and (dev
/// only) the pre-curation factory tree kept as test fixtures. The fixture
/// root qualifies as `factory` — it is the tree those ids were minted for —
/// but resolves last.
fn instrument_roots(paths: &crate::app_paths::AppPaths) -> Vec<InstrumentRoot> {
    let mut roots = paths
        .instrument_roots()
        .into_iter()
        .map(|root| InstrumentRoot {
            tier: root.tier,
            path: root.path,
            fallback: false,
        })
        .collect::<Vec<_>>();
    if let Some(fixtures) = paths.dev_instrument_fixtures_dir() {
        roots.push(InstrumentRoot {
            tier: InstrumentTier::Factory,
            path: fixtures,
            fallback: true,
        });
    }
    roots
}

fn factory_instruments_root(paths: &crate::app_paths::AppPaths) -> PathBuf {
    paths.instruments_dir()
}

/// Every directory that may hold an instrument source, for path-to-name
/// stripping and `ui.lisp` lookup. The browser lists only
/// `AppPaths::instrument_dirs`; this adds the dev fixture root.
pub(in crate::lisp_host) fn instrument_source_roots() -> Vec<PathBuf> {
    instrument_roots(crate::app_paths::app_paths())
        .into_iter()
        .map(|root| root.path)
        .collect()
}

fn parse_instrument_id(name: &str) -> io::Result<Option<(InstrumentTier, &str)>> {
    let trimmed = name.trim_end_matches('/');
    let qualified = InstrumentTier::parse_id(trimmed).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported instrument id '{name}'"),
        )
    })?;
    let path = qualified.as_ref().map(|(_, path)| *path).unwrap_or(trimmed);
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid instrument id '{name}'"),
        ));
    }
    Ok(qualified)
}

// ── Versioned instruments (docs/instrument-versioning-spec.md, user tier:
// docs/patcher-workspace-spec.md §5) ──

/// One factory or user instrument whose `instrument.json` carries `releases`.
struct VersionedInstrument {
    /// `Factory` or `User`: the tier its pinned ids qualify as.
    tier: InstrumentTier,
    /// Logical path of the top folder, e.g. `Synths/Digi Syn`.
    lineage: String,
    /// `current`, or the highest release when the manifest omits it.
    current: u32,
    /// Release number → (release folder, the name it shipped under).
    releases: std::collections::BTreeMap<u32, (PathBuf, String)>,
}

/// The release an instrument id resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentRelease {
    /// The tier the lineage lives in (`Factory` or `User`).
    pub tier: InstrumentTier,
    /// Logical path of the instrument's top folder (its lineage). A pinned id
    /// for this release is `<tier>:<lineage>@<release>`.
    pub lineage: String,
    pub release: u32,
    /// The logical path this release shipped under (`releases[n].name`).
    pub name: String,
    /// The lineage's `current` release.
    pub current: u32,
    /// The release folder: holds `dsp.lisp`, `ui.lisp`, `dsp.layout.json`,
    /// and optionally its own `instrument.json`; its bank is
    /// `<folder>.presets` beside it.
    pub folder: PathBuf,
    /// The `@<release>` the id was written with, if any.
    pub pinned: Option<u32>,
}

type InstrumentReleaseIndex = Vec<VersionedInstrument>;

/// Release manifests of the factory- and user-tier roots, scanned once per
/// root set.
/// Keyed by the roots so temp-root tests never see each other's index.
fn instrument_release_index_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<Vec<PathBuf>, std::sync::Arc<InstrumentReleaseIndex>>>
{
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<Vec<PathBuf>, std::sync::Arc<InstrumentReleaseIndex>>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Drop the cached release index (and the resolved-path memo that may hold
/// release folders). Call after writing any `instrument.json` manifest
/// in-process ([`cut_user_release`] calls it itself).
pub fn invalidate_instrument_release_index() {
    instrument_release_index_cache().lock().unwrap().clear();
    resolved_walk_cache().lock().unwrap().clear();
}

/// Roots that may hold release manifests: the factory tier, the dev fixture
/// root (factory ids), and the user tier, whose manifests the app writes
/// ([`cut_user_release`]). Packages ship none.
fn release_roots(paths: &crate::app_paths::AppPaths) -> Vec<(InstrumentTier, PathBuf)> {
    let mut roots = vec![(InstrumentTier::Factory, factory_instruments_root(paths))];
    roots.extend(paths.dev_instrument_fixtures_dir().map(|root| (InstrumentTier::Factory, root)));
    roots.push((InstrumentTier::User, paths.user_instruments_dir()));
    roots
}

fn instrument_release_index(
    paths: &crate::app_paths::AppPaths,
) -> std::sync::Arc<InstrumentReleaseIndex> {
    let tiered_roots = release_roots(paths);
    let roots: Vec<PathBuf> = tiered_roots.iter().map(|(_, root)| root.clone()).collect();
    if let Some(index) = instrument_release_index_cache().lock().unwrap().get(&roots) {
        return index.clone();
    }
    let mut index = Vec::new();
    for (tier, root) in &tiered_roots {
        scan_release_manifests(tier, root, root, &mut index);
    }
    let index = std::sync::Arc::new(index);
    instrument_release_index_cache()
        .lock()
        .unwrap()
        .insert(roots, index.clone());
    index
}

/// Walk `dir` for manifests with `releases`. A versioned instrument's folder
/// and any instrument folder (`dsp.lisp`) are leaves, so `versions/` is never
/// entered.
fn scan_release_manifests(
    tier: &InstrumentTier,
    root: &Path,
    dir: &Path,
    out: &mut InstrumentReleaseIndex,
) {
    let manifest = dir.join("instrument.json");
    if dir != root && manifest.is_file() {
        match read_release_manifest(tier, root, dir, &manifest) {
            Ok(Some(instrument)) => {
                out.push(instrument);
                return;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("ignoring instrument release manifest '{}': {error}", manifest.display());
                return;
            }
        }
    }
    if dir != root && dir.join("dsp.lisp").is_file() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
        })
        .collect();
    children.sort();
    for child in children {
        scan_release_manifests(tier, root, &child, out);
    }
}

fn read_release_manifest(
    tier: &InstrumentTier,
    root: &Path,
    dir: &Path,
    manifest: &Path,
) -> Result<Option<VersionedInstrument>, String> {
    let source = std::fs::read_to_string(manifest).map_err(|error| error.to_string())?;
    let parsed: InstrumentReleaseManifest =
        serde_json::from_str(&source).map_err(|error| error.to_string())?;
    if parsed.releases.is_empty() {
        return Ok(None);
    }
    let lineage = dir
        .strip_prefix(root)
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let mut releases = std::collections::BTreeMap::new();
    for (release, entry) in parsed.releases {
        let relative = Path::new(&entry.path);
        if relative.as_os_str().is_empty()
            || relative.components().any(|component| {
                !matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            return Err(format!("release {release} has invalid path '{}'", entry.path));
        }
        // `.` normalizes away so the folder's own name (and so its
        // `<folder>.presets` sibling) is the top folder's.
        let folder = relative
            .components()
            .filter(|component| matches!(component, std::path::Component::Normal(_)))
            .fold(dir.to_path_buf(), |folder, component| folder.join(component));
        let name = entry.name.trim_end_matches('/').to_string();
        releases.insert(release, (folder, name));
    }
    let highest = *releases.keys().next_back().expect("non-empty releases");
    let current = parsed.current.unwrap_or(highest);
    if !releases.contains_key(&current) {
        return Err(format!("current release {current} is not listed in releases"));
    }
    Ok(Some(VersionedInstrument { tier: tier.clone(), lineage, current, releases }))
}

/// Resolve an instrument id against the release manifests. `Ok(None)` means
/// the id names no versioned instrument and resolves as it always has.
///
/// - Pinned (`<logical>@n`): the instrument whose top folder (or, failing
///   that, some release name) is `<logical>`; unknown `n` is an error.
/// - Unpinned: the lowest release whose `name` is `<logical>` — unpinned ids
///   predate pinning, so that is the release they were saved against; if
///   only the top folder matches, `current`.
///
/// `factory:` ids and bare names consult the factory manifests; `user:` ids
/// consult the user manifests (patcher-workspace spec §5). A bare name never
/// reaches a user lineage through the index: the runtime spells a user top
/// folder `<logical>/`, which must keep meaning the folder's current files,
/// while an unpinned `user:<logical>` means release 1 (what it was saved
/// against). A pin on a `pkg:` id is an error: packages ship no manifests.
pub fn instrument_release(name: &str) -> io::Result<Option<InstrumentRelease>> {
    instrument_release_with_paths(crate::app_paths::app_paths(), name)
}

pub(in crate::lisp_host) fn instrument_release_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<Option<InstrumentRelease>> {
    let qualified = parse_instrument_id(name)?;
    let (tier, path) = match &qualified {
        Some((tier, path)) => (Some(tier), *path),
        None => (None, name.trim_end_matches('/')),
    };
    let (logical, pinned) = InstrumentTier::split_release(path);
    let index_tier = match tier {
        None | Some(InstrumentTier::Factory) => InstrumentTier::Factory,
        Some(InstrumentTier::User) => InstrumentTier::User,
        Some(InstrumentTier::Package(_)) => {
            return match pinned {
                Some(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("instrument '{name}' pins a release, but package instruments are not versioned"),
                )),
                None => Ok(None),
            };
        }
    };
    let index = instrument_release_index(paths);
    let in_tier = || index.iter().filter(|instrument| instrument.tier == index_tier);
    let by_lineage = || in_tier().find(|instrument| instrument.lineage == logical);
    let by_name = || {
        in_tier()
            .find(|instrument| instrument.releases.values().any(|(_, release_name)| release_name == logical))
    };
    let found = match pinned {
        Some(release) => {
            let Some(instrument) = by_lineage().or_else(by_name) else {
                // A bare name ending in `@<digits>` may just be a legacy
                // name; only a qualified pin is a hard error.
                if qualified.is_none() {
                    return Ok(None);
                }
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("instrument '{name}' pins release {release}, but '{logical}' has no release manifest"),
                ));
            };
            if !instrument.releases.contains_key(&release) {
                let available = instrument
                    .releases
                    .iter()
                    .map(|(n, (_, release_name))| format!("{n} ({release_name})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "instrument '{name}' pins release {release}, which '{}' does not have; available releases: {available}",
                        instrument.lineage
                    ),
                ));
            }
            (instrument, release)
        }
        None => {
            if let Some(instrument) = by_name() {
                let release = instrument
                    .releases
                    .iter()
                    .find(|(_, (_, release_name))| release_name == logical)
                    .map(|(release, _)| *release)
                    .expect("by_name matched a release");
                (instrument, release)
            } else if let Some(instrument) = by_lineage() {
                (instrument, instrument.current)
            } else {
                return Ok(None);
            }
        }
    };
    let (instrument, release) = found;
    let (folder, release_name) = &instrument.releases[&release];
    Ok(Some(InstrumentRelease {
        tier: instrument.tier.clone(),
        lineage: instrument.lineage.clone(),
        release,
        name: release_name.clone(),
        current: instrument.current,
        folder: folder.clone(),
        pinned,
    }))
}

/// The pinned id of a release: `<tier>:<lineage>@<release>`, what saves write.
fn pinned_release_id(release: &InstrumentRelease) -> String {
    release.tier.qualify(&format!("{}@{}", release.lineage, release.release))
}

/// The release whose folder is exactly `folder` (the top folder or a
/// `versions/<n>` folder), pinned to itself.
fn instrument_release_for_folder_with_paths(
    paths: &crate::app_paths::AppPaths,
    folder: &Path,
) -> Option<InstrumentRelease> {
    instrument_release_index(paths).iter().find_map(|instrument| {
        instrument
            .releases
            .iter()
            .find(|(_, (release_folder, _))| release_folder == folder)
            .map(|(release, (release_folder, name))| InstrumentRelease {
                tier: instrument.tier.clone(),
                lineage: instrument.lineage.clone(),
                release: *release,
                name: name.clone(),
                current: instrument.current,
                folder: release_folder.clone(),
                pinned: Some(*release),
            })
    })
}

/// The pinned id for an instrument folder that is a release folder
/// (`…/Digi Syn` or `…/Digi Syn/versions/1`, factory or user tier); `None`
/// for any other folder.
/// Anything that turns a source path back into an id must use this before
/// stripping roots: a `versions/<n>` path is not an id.
pub fn instrument_release_id_for_folder(folder: &Path) -> Option<String> {
    instrument_release_for_folder_with_paths(crate::app_paths::app_paths(), folder)
        .map(|release| pinned_release_id(&release))
}

/// The logical path, under the user tier (and a package's `presets/factory`),
/// that holds a release's user presets: its `name`. That keeps an overlay
/// saved before the lineage was versioned
/// (`<user>/instruments/Synths/Digi Drift.presets`) on the release that
/// shipped as Digi Drift, and gives the current release the lineage path
/// whenever its name is the lineage. A later release that reuses an earlier
/// release's name gets `<name>@<n>`, so no two releases (two parameter
/// layouts) ever share a user bank.
fn release_user_preset_logical(
    paths: &crate::app_paths::AppPaths,
    release: &InstrumentRelease,
) -> String {
    let first_with_name =
        instrument_release_with_paths(paths, &release.tier.qualify(&release.name))
            .ok()
            .flatten();
    if first_with_name
        .is_some_and(|first| first.lineage == release.lineage && first.release == release.release)
    {
        release.name.clone()
    } else {
        format!("{}@{}", release.name, release.release)
    }
}

/// The id a *new* track or rack slot gets when the user picks `name` (a
/// browser row, a dropped instrument): the spec's "new tracks use `current`".
/// A versioned instrument is pinned — to `current` when `name` is the
/// lineage's own top folder, otherwise to the release `name` resolves to (an
/// explicit pin, or a legacy name). Everything else comes back unchanged.
/// Legacy ids in files do not go through this; they resolve (and qualify) to
/// the release they were saved against.
pub fn pin_instrument_for_new_track(name: &str) -> String {
    pin_instrument_for_new_track_with_paths(crate::app_paths::app_paths(), name)
}

fn pin_instrument_for_new_track_with_paths(paths: &crate::app_paths::AppPaths, name: &str) -> String {
    let Ok(Some(mut release)) = instrument_release_with_paths(paths, name) else {
        // A bare name that lands on a user release folder (a browser row
        // `<logical>/`) pins to that folder's release.
        return release_for_resolved_source(paths, name)
            .map(|release| pinned_release_id(&release))
            .unwrap_or_else(|| name.to_string());
    };
    let path = match parse_instrument_id(name) {
        Ok(Some((_, path))) => path,
        _ => name.trim_end_matches('/'),
    };
    if release.pinned.is_none() && InstrumentTier::split_release(path).0 == release.lineage {
        release.release = release.current;
    }
    pinned_release_id(&release)
}

/// `<tier>:<lineage>` for any id of a versioned instrument (any release, any
/// legacy name); `None` otherwise. Favorites and "which browser row is this
/// track" key on this so they survive new releases and renames.
pub fn instrument_lineage_id(name: &str) -> Option<String> {
    instrument_lineage_id_with_paths(crate::app_paths::app_paths(), name)
}

fn instrument_lineage_id_with_paths(paths: &crate::app_paths::AppPaths, name: &str) -> Option<String> {
    let release = match instrument_release_with_paths(paths, name) {
        Ok(Some(release)) => Some(release),
        // The index never takes a bare name to a user lineage; a bare name
        // that resolves onto a user release folder (a runtime/browser
        // `<logical>/`) still belongs to that lineage.
        Ok(None) if matches!(parse_instrument_id(name), Ok(None)) => {
            release_for_resolved_source(paths, name)
        }
        _ => None,
    };
    release.map(|release| release.tier.qualify(&release.lineage))
}

/// The release whose folder holds the source `name` resolves to, when that
/// is a release folder. Covers spellings the index does not answer for
/// (bare names landing on a user lineage's top folder).
fn release_for_resolved_source(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> Option<InstrumentRelease> {
    let source = resolve_instrument_storage_path_with_paths(paths, name, "lisp").ok()?;
    if source.file_name().and_then(|file| file.to_str()) != Some("dsp.lisp") || !source.is_file() {
        return None;
    }
    instrument_release_for_folder_with_paths(paths, source.parent()?)
}

/// One release folder (factory or user tier) with the ids that load it, for
/// dispatching its `ui.lisp` by an engine's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentReleaseIds {
    pub folder: PathBuf,
    /// The pinned id, `<tier>:<lineage>@<release>`.
    pub id: String,
    /// Every other spelling that resolves to this release: the pin through an
    /// old name, and the unpinned lineage/name forms (bare, `factory:`, with
    /// and without the trailing `/`, and their leaf) that land here.
    pub aliases: Vec<String>,
}

pub fn instrument_release_ids() -> Vec<InstrumentReleaseIds> {
    instrument_release_ids_with_paths(crate::app_paths::app_paths())
}

fn instrument_release_ids_with_paths(paths: &crate::app_paths::AppPaths) -> Vec<InstrumentReleaseIds> {
    with_leaf_walk_snapshot(|| release_ids_from_index(paths))
}

fn release_ids_from_index(paths: &crate::app_paths::AppPaths) -> Vec<InstrumentReleaseIds> {
    let index = instrument_release_index(paths);
    let mut out = Vec::new();
    for instrument in index.iter() {
        let mut logicals: Vec<&str> = vec![instrument.lineage.as_str()];
        for (_, name) in instrument.releases.values() {
            if !logicals.contains(&name.as_str()) {
                logicals.push(name);
            }
        }
        for (release, (folder, release_name)) in &instrument.releases {
            let source = folder.join("dsp.lisp");
            let tier = &instrument.tier;
            let id = tier.qualify(&format!("{}@{release}", instrument.lineage));
            let mut candidates = Vec::new();
            for logical in &logicals {
                // A pin is only ever written on the lineage or, by hand, on
                // the name the release shipped under.
                if *logical == release_name.as_str() {
                    candidates.push(tier.qualify(&format!("{logical}@{release}")));
                }
                candidates.push(tier.qualify(logical));
                candidates.push(format!("{}/", tier.qualify(logical)));
                candidates.push(logical.to_string());
                candidates.push(format!("{logical}/"));
                if let Some(leaf) = Path::new(logical).file_name().and_then(|leaf| leaf.to_str()) {
                    if leaf != *logical {
                        candidates.push(leaf.to_string());
                        candidates.push(format!("{leaf}/"));
                    }
                }
            }
            let mut aliases: Vec<String> = Vec::new();
            for candidate in candidates {
                if candidate == id || aliases.contains(&candidate) {
                    continue;
                }
                let resolves_here = resolve_instrument_storage_path_with_paths(paths, &candidate, "lisp")
                    .is_ok_and(|resolved| resolved == source);
                if resolves_here {
                    aliases.push(candidate);
                }
            }
            out.push(InstrumentReleaseIds { folder: folder.clone(), id, aliases });
        }
    }
    out
}

/// A storage file of a release: its `dsp.lisp`, or the `<folder>.<ext>`
/// sibling (`versions/1.presets`, `Digi Syn.presets`).
fn release_storage_path(folder: &Path, extension: &str) -> PathBuf {
    if extension == "lisp" {
        return folder.join("dsp.lisp");
    }
    let file_name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    folder.with_file_name(format!("{file_name}.{extension}"))
}

/// One instrument root's non-hidden entries by file name, for the leaf-name
/// fallback: `folders` maps a folder's name to its `dsp.lisp`, for folders
/// that have one; `files` maps every non-directory's name to its path.
#[derive(Default)]
struct RootLeafIndex {
    folders: std::collections::HashMap<String, Vec<PathBuf>>,
    files: std::collections::HashMap<String, Vec<PathBuf>>,
}

impl RootLeafIndex {
    fn build(root: &Path) -> Self {
        fn walk(dir: &Path, index: &mut RootLeafIndex) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|name| name.to_str()).map(str::to_owned);
                if name.as_deref().is_some_and(|name| name.starts_with('.')) {
                    continue;
                }
                if path.is_dir() {
                    let dsp = path.join("dsp.lisp");
                    if let Some(name) = name.filter(|_| dsp.exists()) {
                        index.folders.entry(name).or_default().push(dsp);
                    }
                    walk(&path, index);
                } else if let Some(name) = name {
                    index.files.entry(name).or_default().push(path);
                }
            }
        }
        let mut index = Self::default();
        walk(root, &mut index);
        index
    }
}

thread_local! {
    /// Root indexes shared by the leaf-name lookups of one batch; `None`
    /// outside [`with_leaf_walk_snapshot`], where every lookup walks.
    static LEAF_WALK_SNAPSHOT: std::cell::RefCell<
        Option<std::collections::HashMap<PathBuf, std::rc::Rc<RootLeafIndex>>>,
    > = const { std::cell::RefCell::new(None) };
}

/// Run `f` with each root's tree walked at most once for leaf-name lookups.
/// For batches that resolve many spellings back to back
/// ([`instrument_release_ids_with_paths`] resolves ~10 per release); the
/// tree must not change meanwhile.
fn with_leaf_walk_snapshot<T>(f: impl FnOnce() -> T) -> T {
    /// Drops the snapshot this call installed, even if `f` unwinds.
    struct Installed(bool);
    impl Drop for Installed {
        fn drop(&mut self) {
            if self.0 {
                LEAF_WALK_SNAPSHOT.with(|snapshot| snapshot.borrow_mut().take());
            }
        }
    }
    let _installed = Installed(LEAF_WALK_SNAPSHOT.with(|snapshot| {
        let mut snapshot = snapshot.borrow_mut();
        let install = snapshot.is_none();
        if install {
            *snapshot = Some(Default::default());
        }
        install
    }));
    f()
}

/// `root`'s leaf index: the batch's shared one inside
/// [`with_leaf_walk_snapshot`], otherwise a fresh walk.
fn root_leaf_index(root: &Path) -> std::rc::Rc<RootLeafIndex> {
    LEAF_WALK_SNAPSHOT.with(|snapshot| match snapshot.borrow_mut().as_mut() {
        Some(indexes) => indexes
            .entry(root.to_path_buf())
            .or_insert_with(|| std::rc::Rc::new(RootLeafIndex::build(root)))
            .clone(),
        None => std::rc::Rc::new(RootLeafIndex::build(root)),
    })
}

fn resolve_instrument_storage_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    extension: &str,
) -> io::Result<PathBuf> {
    fn resolve_in_root(
        root: &Path,
        logical_name: &str,
        extension: &str,
        allow_leaf_fallback: bool,
    ) -> io::Result<Option<PathBuf>> {
        let trimmed = logical_name.trim_end_matches('/');
        let exact = root.join(format!("{trimmed}.{extension}"));
        if exact.exists() {
            return Ok(Some(exact));
        }
        if extension == "lisp" {
            let dsp = root.join(trimmed).join("dsp.lisp");
            if dsp.exists() {
                return Ok(Some(dsp));
            }
        }

        if !allow_leaf_fallback {
            return Ok(None);
        }

        let basename = Path::new(trimmed)
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or(trimmed);
        let file_name = format!("{basename}.{extension}");
        let index = root_leaf_index(root);
        let mut matches = Vec::new();
        if extension == "lisp" {
            matches.extend(index.folders.get(basename).into_iter().flatten().cloned());
        }
        matches.extend(index.files.get(&file_name).into_iter().flatten().cloned());
        matches.sort_by_key(|path| path.to_string_lossy().to_lowercase());
        matches.dedup();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Ambiguous instrument '{logical_name}': found multiple matching instrument sources under {}",
                    root.display()
                ),
            )),
        }
    }

    let qualified = parse_instrument_id(name)?;
    let logical_name = qualified
        .as_ref()
        .map(|(_, logical_name)| *logical_name)
        .unwrap_or_else(|| name.trim_end_matches('/'));

    // The walk cache is consulted before the roots are enumerated: hot
    // callers (glyph feeds re-read sources every reactive tick) must pay one
    // hash lookup and an `exists()`, not the package-catalog scan.
    let cache_key = (name.to_string(), extension.to_string());
    if let Some(cached) = resolved_walk_cache().lock().unwrap().get(&cache_key).cloned() {
        if cached.exists() {
            return Ok(cached);
        }
        resolved_walk_cache().lock().unwrap().remove(&cache_key);
    }

    // A versioned factory instrument resolves to its release folder; every
    // sibling lookup (ui.lisp, layout, instrument.json, bank) then follows
    // that folder.
    if let Some(release) = instrument_release_with_paths(paths, name)? {
        let resolved = release_storage_path(&release.folder, extension);
        if resolved.exists() {
            resolved_walk_cache()
                .lock()
                .unwrap()
                .insert(cache_key, resolved.clone());
        }
        return Ok(resolved);
    }

    // A qualified id names an exact logical path. `factory:` ids search the
    // factory tier, then the user tier, then fallback roots: they keep working
    // after an instrument leaves the shipped factory set and lives on as a
    // user-tier copy under the same path (the 2026-09 curation); saving
    // re-qualifies them through `qualify_instrument_id`. `user:` ids never
    // fall through — a user id whose copy is missing must still resolve into
    // the user tier, otherwise a save would write over shipped factory content
    // and requalify the project's instrument as read-only. `pkg:` ids resolve
    // only inside their own package: instruments never shadow across tiers,
    // so a missing package instrument is missing, not somebody else's. Bare
    // names keep the legacy leaf-name walk within each root, factory and user
    // tiers before packages.
    let all_roots = instrument_roots(paths);
    let mut ordered: Vec<&InstrumentRoot> = Vec::with_capacity(all_roots.len());
    match &qualified {
        Some((InstrumentTier::User, _)) => {
            ordered.extend(all_roots.iter().filter(|r| !r.fallback && r.tier == InstrumentTier::User));
        }
        Some((tier @ InstrumentTier::Package(_), _)) => {
            ordered.extend(all_roots.iter().filter(|r| !r.fallback && r.tier == *tier));
            if ordered.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("instrument '{name}' belongs to a package that is not installed"),
                ));
            }
        }
        Some((InstrumentTier::Factory, _)) => {
            ordered.extend(all_roots.iter().filter(|r| !r.fallback && r.tier == InstrumentTier::Factory));
            ordered.extend(all_roots.iter().filter(|r| !r.fallback && r.tier == InstrumentTier::User));
            ordered.extend(all_roots.iter().filter(|r| r.fallback));
        }
        None => {
            ordered.extend(all_roots.iter().filter(|r| !r.fallback));
            ordered.extend(all_roots.iter().filter(|r| r.fallback));
        }
    }

    let remember = |resolved: PathBuf| {
        resolved_walk_cache()
            .lock()
            .unwrap()
            .insert(cache_key.clone(), resolved.clone());
        resolved
    };

    if qualified.is_some() {
        // The instrument lives wherever its source is; every other file of it
        // (bank, metadata) is looked up beside that source only, so a factory
        // instrument with no shipped bank still keeps its bank slot in the
        // factory tier rather than borrowing a user-tier file of the same name.
        let home = ordered
            .iter()
            .find(|root| {
                matches!(
                    resolve_in_root(&root.path, logical_name, "lisp", false),
                    Ok(Some(_))
                )
            })
            .copied()
            .unwrap_or(ordered[0]);
        if let Some(resolved) = resolve_in_root(&home.path, logical_name, extension, false)? {
            return Ok(remember(resolved));
        }
        return Ok(home.path.join(format!("{logical_name}.{extension}")));
    }

    for root in &ordered {
        if let Some(resolved) = resolve_in_root(&root.path, logical_name, extension, true)? {
            return Ok(remember(resolved));
        }
    }

    Ok(ordered[0]
        .path
        .join(format!("{logical_name}.{extension}")))
}

#[cfg(test)]
pub fn instrument_presets_with_paths_for_tests(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<std::sync::Arc<Vec<InstrumentPreset>>> {
    cached_instrument_presets_with_paths(paths, name)
}

#[cfg(test)]
pub fn instrument_source_path_with_paths_for_tests(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    resolve_instrument_storage_path_with_paths(paths, name, "lisp")
}

pub(in crate::lisp_host) fn resolve_instrument_storage_path(name: &str, extension: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path_with_paths(crate::app_paths::app_paths(), name, extension)
}

/// Return the stable project id for an instrument. Legacy bare names prefer
/// factory content when both tiers contain the same name; only a missing
/// factory source falls through to the user tier.
pub fn qualify_instrument_id(name: &str) -> io::Result<String> {
    qualify_instrument_id_with_paths(crate::app_paths::app_paths(), name)
}

fn qualify_instrument_id_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<String> {
    let source = resolve_instrument_storage_path_with_paths(paths, name, "lisp")?;
    if !source.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("instrument '{name}' does not exist"),
        ));
    }
    // Saves always pin (spec §Ids): a versioned instrument qualifies to
    // `factory:<lineage>@<release>` for the release its id resolves to, so the
    // file keeps naming that exact release after later ones ship or rename
    // the lineage. A release folder's own path (`…/versions/1`) is never an
    // id. Project load runs this too, so a legacy unpinned id is pinned to
    // the release it already resolved to and the next save writes the pin.
    if let Some(release) = instrument_release_with_paths(paths, name)? {
        return Ok(pinned_release_id(&release));
    }
    // A spelling the index does not answer for (a bare `<logical>/` on a user
    // lineage's top folder) still pins to the release folder it landed on.
    if let Some(release) = source
        .parent()
        .filter(|_| source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp"))
        .and_then(|folder| instrument_release_for_folder_with_paths(paths, folder))
    {
        return Ok(pinned_release_id(&release));
    }
    for InstrumentRoot { tier, path: root, .. } in instrument_roots(paths) {
        if let Ok(relative) = source.strip_prefix(&root) {
            let logical = if relative.file_name().and_then(|part| part.to_str()) == Some("dsp.lisp") {
                relative.parent().unwrap_or(relative).to_path_buf()
            } else {
                relative.with_extension("")
            };
            let logical = logical.to_string_lossy().replace('\\', "/");
            return Ok(tier.qualify(&logical));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("instrument source '{}' is outside the configured tiers", source.display()),
    ))
}

pub(in crate::lisp_host) fn resolve_instrument_folder_path(name: &str) -> io::Result<PathBuf> {
    let source = resolve_instrument_storage_path(name, "lisp")?;
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Resolved folder-style instrument '{name}' has no parent directory"),
            )
        })
    } else {
        Ok(source.with_extension(""))
    }
}

pub fn instrument_source_path(name: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path(name, "lisp")
}

pub(in crate::lisp_host) fn instrument_metadata_path_for_source_path(source: &Path) -> io::Result<PathBuf> {
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source
            .parent()
            .map(|parent| parent.join("instrument.json"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Resolved folder-style instrument source '{}' has no parent directory",
                        source.display()
                    ),
                )
            })
    } else {
        Ok(source.with_extension("instrument.json"))
    }
}

pub fn instrument_metadata_path(name: &str) -> io::Result<PathBuf> {
    let source = instrument_source_path(name)?;
    instrument_metadata_path_for_source_path(&source)
}

pub fn load_instrument_run_mode(name: &str) -> io::Result<CustomInstrumentRunMode> {
    let path = instrument_metadata_path(name)?;
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CustomInstrumentRunMode::Instrument);
        }
        Err(error) => return Err(error),
    };
    let metadata: InstrumentMetadataFile = serde_json::from_str(&source).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "failed to parse instrument metadata '{}': {error}",
                path.display()
            ),
        )
    })?;
    CustomInstrumentRunMode::parse(&metadata.run_mode).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid instrument run_mode '{}'", metadata.run_mode),
        )
    })
}

pub fn save_instrument_run_mode(name: &str, run_mode: CustomInstrumentRunMode) -> io::Result<()> {
    let source = writable_instrument_source_path(name)?;
    let path = instrument_metadata_path_for_source_path(&source)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut metadata = match std::fs::read_to_string(&path) {
        Ok(source) => serde_json::from_str::<InstrumentMetadataFile>(&source)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => InstrumentMetadataFile {
            version: 1, run_mode: run_mode.as_str().to_string(), voice_controls: None,
            current: None, releases: None, deprecated: false, replaced_by: None,
        },
        Err(error) => return Err(error),
    };
    metadata.run_mode = run_mode.as_str().to_string();
    let json = serde_json::to_string_pretty(&metadata).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to encode instrument metadata: {error}"),
        )
    })?;
    std::fs::write(path, format!("{json}\n"))
}

/// Strip a content root from `parent`: any AppPaths root in `roots`, or the
/// bare relative dir name (`instruments/…`, `effects/…`) that production path
/// strings still carry (patcher buffers, UI state, saved descriptors).
fn strip_source_root(parent: &Path, roots: &[PathBuf], relative_dir: &str) -> Option<PathBuf> {
    for root in roots {
        if let Ok(rel) = parent.strip_prefix(root) {
            return Some(rel.to_path_buf());
        }
    }
    parent
        .strip_prefix(relative_dir)
        .ok()
        .map(Path::to_path_buf)
}

pub(in crate::lisp_host) fn instrument_name_from_source_path(path: &Path) -> Option<String> {
    if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        if let Some(parent) = path.parent() {
            // A frozen release folder (`…/versions/<n>`) is not an id; name it
            // by its pin. The top folder keeps its plain logical name, which
            // resolves to the same release.
            if parent.parent().and_then(|versions| versions.file_name()).and_then(|name| name.to_str())
                == Some("versions")
            {
                if let Some(id) = instrument_release_id_for_folder(parent) {
                    return Some(id);
                }
            }
            // A package instrument's name is always qualified: its bare
            // relative path would resolve factory-first and land elsewhere.
            for root in instrument_roots(crate::app_paths::app_paths()) {
                if !root.tier.is_package() {
                    continue;
                }
                if let Ok(rel) = parent.strip_prefix(&root.path) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if !rel.is_empty() {
                        return Some(format!("{}/", root.tier.qualify(&rel)));
                    }
                }
            }
            if let Some(rel) = strip_source_root(parent, &instrument_source_roots(), "instruments")
            {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !rel.is_empty() {
                    return Some(format!("{rel}/"));
                }
            }
        }
    }

    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
}

pub(in crate::lisp_host) fn source_name_from_path(kind: &CompileKind, path: &Path) -> Option<String> {
    match kind {
        CompileKind::Instrument => instrument_name_from_source_path(path),
        CompileKind::Effect => {
            if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
                let parent = path.parent()?;
                for root in crate::app_paths::app_paths().effect_roots() {
                    if !root.tier.is_package() {
                        continue;
                    }
                    if let Ok(rel) = parent.strip_prefix(&root.path) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if !rel.is_empty() {
                            return Some(root.tier.qualify(&rel));
                        }
                    }
                }
                strip_source_root(
                    parent,
                    &crate::app_paths::app_paths().effect_dirs(),
                    "effects",
                )
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            } else {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
            }
        }
    }
}

/// Preset banks are user data, not instrument source: saving presets on a
/// factory instrument must not require forking it. Factory-qualified ids keep
/// their writable bank in the user tier under the same logical path — exactly
/// where legacy bare names wrote it. On load that user bank is an *overlay* on
/// the factory-shipped bank: the two are merged by preset name, user entries
/// shadowing factory ones (see [`merge_preset_banks`]).
fn user_tier_preset_path(paths: &crate::app_paths::AppPaths, logical_name: &str) -> PathBuf {
    paths
        .user_instruments_dir()
        .join(format!("{logical_name}.presets"))
}

/// Where a package instrument's user presets live. Packages are read-only
/// like the factory tier, but their logical paths would collide with the
/// user's own instruments under `user_instruments_dir`, so each package gets
/// its own overlay tree under a hidden directory that no instrument walk
/// lists.
fn package_tier_preset_path(
    paths: &crate::app_paths::AppPaths,
    package_prefix: &str,
    logical_name: &str,
) -> PathBuf {
    paths
        .user_instruments_dir()
        .join(".package-presets")
        .join(package_prefix)
        .join(format!("{logical_name}.presets"))
}

/// The bank files that make up one instrument's preset list.
///
/// `base` is the bank next to the resolved source (the factory-shipped bank for
/// factory ids, the instrument's own bank otherwise). `user_overlay` is the
/// user-tier bank for factory ids — the file saves go to — and `None` for
/// instruments whose `base` is already writable.
struct PresetBankPaths {
    base: PathBuf,
    /// Banks shipped by installed packages for this instrument
    /// (`<package>/presets/<factory|user>/<logical>.presets`), in package
    /// load order. Merged between `base` and `user_overlay`: a pack can add
    /// or reshape presets for a factory synth, and the user's own saves
    /// still win.
    package_overlays: Vec<PathBuf>,
    user_overlay: Option<PathBuf>,
}

impl PresetBankPaths {
    fn all(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.base.clone()];
        paths.extend(self.package_overlays.iter().cloned());
        if let Some(overlay) = &self.user_overlay {
            paths.push(overlay.clone());
        }
        paths
    }
}

/// Where a package would ship presets for `name`: the tier directory the
/// id belongs to (`factory` for factory ids and legacy bare names, which
/// resolve factory-first; `user` for user ids) under the package's
/// `presets/`. Package instruments carry their bank beside the source, so
/// they have no overlay slot.
fn package_preset_overlay_relative(name: &str) -> io::Result<Option<PathBuf>> {
    let (tier_dir, logical) = match parse_instrument_id(name)? {
        Some((InstrumentTier::Package(_), _)) => return Ok(None),
        Some((InstrumentTier::User, logical)) => ("user", logical),
        Some((InstrumentTier::Factory, logical)) => ("factory", logical),
        None => ("factory", name.trim_end_matches('/')),
    };
    Ok(Some(Path::new(tier_dir).join(format!("{logical}.presets"))))
}

fn package_preset_overlays_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<Vec<PathBuf>> {
    // A versioned instrument's pack banks are keyed like its user bank: by
    // the release's name, so a pack written for Digi Drift stays on release 1.
    let relative = match instrument_release_with_paths(paths, name)? {
        Some(release) => Path::new(if release.tier == InstrumentTier::User { "user" } else { "factory" })
            .join(format!("{}.presets", release_user_preset_logical(paths, &release))),
        None => match package_preset_overlay_relative(name)? {
            Some(relative) => relative,
            None => return Ok(Vec::new()),
        },
    };
    Ok(paths
        .package_catalog()
        .ordered()
        .filter_map(|package| package.content_dir("presets"))
        .map(|dir| dir.join(&relative))
        .filter(|path| path.is_file())
        .collect())
}

fn instrument_preset_bank_paths_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PresetBankPaths> {
    let base = resolve_instrument_storage_path_with_paths(paths, name, "presets")?;
    // The overlay is wherever a save would go, whenever that is not the bank
    // the read resolved to. That covers explicit `factory:` ids *and* legacy
    // bare names (the engine-registry form, e.g. `factory/digiwave/`), which
    // resolve factory-first on read yet save into the user tier: without this
    // a bare-named factory instrument's saved presets never show up on load.
    let user_overlay =
        Some(instrument_preset_save_path_with_paths(paths, name)?).filter(|overlay| *overlay != base);
    let package_overlays = package_preset_overlays_with_paths(paths, name)?;
    Ok(PresetBankPaths { base, package_overlays, user_overlay })
}

/// The factory-shipped (or, for user instruments, the only) bank path. The
/// user overlay for factory ids is *not* consulted here; use
/// [`load_instrument_presets_shared`] for the merged list.
pub(in crate::lisp_host) fn instrument_preset_path(name: &str) -> io::Result<PathBuf> {
    instrument_preset_path_with_paths(crate::app_paths::app_paths(), name)
}

fn instrument_preset_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    Ok(instrument_preset_bank_paths_with_paths(paths, name)?.base)
}

/// Read one bank file. `Ok(None)` when the file does not exist.
fn read_preset_bank(path: &Path) -> io::Result<Option<Vec<InstrumentPreset>>> {
    match std::fs::read_to_string(path) {
        Ok(src) => {
            let bank: InstrumentPresetBank = serde_json::from_str(&src).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse preset bank '{}': {e}", path.display()),
                )
            })?;
            Ok(Some(bank.presets))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Merge a factory bank with the user overlay: the result is `base ∪ overlay`
/// with an overlay preset replacing the base preset of the same name, sorted by
/// name like every saved bank. Factory presets are never deleted by the overlay
/// (there is no delete path today; tombstones would live here if one lands).
pub fn merge_preset_banks(
    base: &[InstrumentPreset],
    overlay: &[InstrumentPreset],
) -> Vec<InstrumentPreset> {
    let mut merged = base
        .iter()
        .map(|preset| (preset.name.clone(), preset.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for preset in overlay {
        merged.insert(preset.name.clone(), preset.clone());
    }
    merged.into_values().collect()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PresetBankCacheKey {
    user_instruments_dir: PathBuf,
    instrument_name: String,
}

/// Merged preset lists keyed by instrument, with no freshness check on a warm
/// hit: a hit costs one hash lookup and an `Arc` clone, never a `stat`.
/// Staleness is therefore handled by *explicit* invalidation — every write path
/// in this process (preset saves, instrument-source saves, instrument moves,
/// and the patch-fork bank materialization via
/// [`invalidate_instrument_preset_bank_cache_at`]) drops every entry that read
/// the written file. Each entry remembers *all* the files it was merged from
/// (factory base plus user overlay), so invalidating either one drops it.
/// Edits made by another process to a bank this process has already read are
/// not observed until something invalidates it. Instruments with no bank on
/// disk at all are deliberately *not* cached, so a bank that appears later (a
/// fork, an external copy) is picked up without an invalidation hook.
#[derive(Default)]
struct PresetBankCache {
    entries: std::collections::HashMap<PresetBankCacheKey, PresetBankCacheEntry>,
}

struct PresetBankCacheEntry {
    paths: Vec<PathBuf>,
    presets: std::sync::Arc<Vec<InstrumentPreset>>,
}

impl PresetBankCache {
    fn get(&self, key: &PresetBankCacheKey) -> Option<std::sync::Arc<Vec<InstrumentPreset>>> {
        self.entries.get(key).map(|entry| entry.presets.clone())
    }

    fn insert(
        &mut self,
        key: PresetBankCacheKey,
        paths: Vec<PathBuf>,
        presets: std::sync::Arc<Vec<InstrumentPreset>>,
    ) {
        self.entries.insert(key, PresetBankCacheEntry { paths, presets });
    }

    fn invalidate_path(&mut self, path: &Path) {
        let dropped: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.paths.iter().any(|p| p == path))
            .map(|(key, _)| key.instrument_name.clone())
            .collect();
        self.entries
            .retain(|_, entry| !entry.paths.iter().any(|p| p == path));
        // A bank appearing where no cached entry pointed (a new instrument's
        // first bank) has no name to drop: re-index everything.
        if dropped.is_empty() {
            refresh_preset_name_index();
        } else {
            forget_indexed_preset_names(&dropped);
        }
    }

    fn invalidate_key_and_path(&mut self, key: &PresetBankCacheKey, path: Option<&Path>) {
        let removed = self.entries.remove(key);
        let mut invalidated_paths = removed.map(|entry| entry.paths).unwrap_or_default();
        if let Some(path) = path {
            if !invalidated_paths.iter().any(|p| p == path) {
                invalidated_paths.push(path.to_path_buf());
            }
        }
        let mut dropped = vec![key.instrument_name.clone()];
        dropped.extend(
            self.entries
                .iter()
                .filter(|(_, entry)| entry.paths.iter().any(|p| invalidated_paths.contains(p)))
                .map(|(key, _)| key.instrument_name.clone()),
        );
        self.entries.retain(|_, entry| {
            !entry
                .paths
                .iter()
                .any(|p| invalidated_paths.contains(p))
        });
        forget_indexed_preset_names(&dropped);
    }
}

// ── Preset name index ───────────────────────────────────────────────────
// Every saved instrument's preset names, for search (the command palette
// finds a preset by name inside its instrument). Resolving where a bank
// would live costs a directory walk, and instruments without a bank are
// never kept in the bank cache, so asking each instrument on demand stalls
// the UI. The index is built on a background thread at startup and when the
// content library changes on disk, records "no presets" too, and drops just
// the instruments whose bank a save or watcher invalidates (they reload on
// their next lookup).

#[derive(Default)]
struct PresetNameIndex {
    names: std::collections::HashMap<String, std::sync::Arc<Vec<String>>>,
    /// A rebuild is running; `rerun` asks it to go again when it finishes.
    building: bool,
    rerun: bool,
}

/// Moves when the index's names are replaced or dropped, so a cache built
/// from it (the command palette's preset search) knows to re-read it. An
/// atomic: the UI reads it every tick, off the index lock.
static PRESET_NAME_INDEX_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn preset_name_index() -> &'static std::sync::Mutex<PresetNameIndex> {
    static INDEX: std::sync::OnceLock<std::sync::Mutex<PresetNameIndex>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| std::sync::Mutex::new(PresetNameIndex::default()))
}

/// The index key: the Instruments tree names engines with a trailing slash
/// (`Drums/Modal Kick/`); banks are keyed without it.
fn preset_index_key(name: &str) -> &str {
    name.trim_end_matches('/')
}

/// An instrument's preset names (its factory and user presets, merged) from
/// the index, without touching the disk once the index has it. A name the
/// index has not seen is loaded now and kept.
///
/// While the startup build is still running, a name it has not reached yet
/// answers empty (uncached) rather than parsing banks on the caller's (UI)
/// thread in contention with the builder.
pub fn indexed_instrument_preset_names(name: &str) -> std::sync::Arc<Vec<String>> {
    let key = preset_index_key(name);
    {
        let index = preset_name_index().lock().unwrap();
        if let Some(names) = index.names.get(key) {
            return names.clone();
        }
        if index.building {
            return std::sync::Arc::new(Vec::new());
        }
    }
    let names = std::sync::Arc::new(load_instrument_preset_names(key).unwrap_or_default());
    preset_name_index().lock().unwrap().names.insert(key.to_string(), names.clone());
    names
}

/// Drops instruments from the index after their bank changed; each reloads
/// on its next lookup.
fn forget_indexed_preset_names(instruments: &[String]) {
    let mut index = preset_name_index().lock().unwrap();
    for instrument in instruments {
        index.names.remove(preset_index_key(instrument));
    }
    PRESET_NAME_INDEX_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The preset name index's generation: moves when a rebuild lands or a
/// bank change drops names (a lookup reloading a dropped name does not).
pub fn preset_name_index_generation() -> u64 {
    PRESET_NAME_INDEX_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}


/// Rebuilds the index on a background thread from every saved instrument
/// (startup, a bank change, the content library changing on disk). The
/// current index keeps answering until the new one replaces it.
pub fn refresh_preset_name_index() {
    {
        let mut index = preset_name_index().lock().unwrap();
        if index.building {
            index.rerun = true;
            return;
        }
        index.building = true;
    }
    let spawned = std::thread::Builder::new().name("preset-name-index".into()).spawn(|| loop {
        let mut names = std::collections::HashMap::new();
        for instrument in list_saved_instruments() {
            let key = preset_index_key(&instrument).to_string();
            let presets = load_instrument_preset_names(&key).unwrap_or_default();
            names.insert(key, std::sync::Arc::new(presets));
        }
        let mut index = preset_name_index().lock().unwrap();
        // Replace rather than merge: instruments that went away drop out.
        index.names = names;
        PRESET_NAME_INDEX_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if std::mem::take(&mut index.rerun) {
            continue;
        }
        index.building = false;
        break;
    });
    if spawned.is_err() {
        preset_name_index().lock().unwrap().building = false;
    }
}

fn preset_bank_cache() -> &'static std::sync::Mutex<PresetBankCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<PresetBankCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(PresetBankCache::default()))
}

fn preset_bank_cache_key(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> PresetBankCacheKey {
    PresetBankCacheKey {
        user_instruments_dir: paths.user_instruments_dir(),
        instrument_name: name.to_string(),
    }
}

fn cached_instrument_presets_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<std::sync::Arc<Vec<InstrumentPreset>>> {
    let key = preset_bank_cache_key(paths, name);
    let mut cache = preset_bank_cache().lock().unwrap();
    if let Some(presets) = cache.get(&key) {
        return Ok(presets);
    }

    // Resolve and read while holding the cache lock so a concurrent save cannot
    // publish a new bank and then have this load install stale contents over it.
    let bank_paths = instrument_preset_bank_paths_with_paths(paths, name)?;
    let mut base = read_preset_bank(&bank_paths.base)?;
    for overlay in &bank_paths.package_overlays {
        if let Some(pack) = read_preset_bank(overlay)? {
            base = Some(match base {
                Some(base) => merge_preset_banks(&base, &pack),
                None => pack,
            });
        }
    }
    let overlay = match &bank_paths.user_overlay {
        Some(overlay) => read_preset_bank(overlay)?,
        None => None,
    };
    let presets = match (base, overlay) {
        // No bank anywhere is not cached: there is no entry to invalidate
        // later, so caching it would make "this instrument has no presets"
        // permanent for the life of the process even after a bank appears.
        (None, None) => return Ok(std::sync::Arc::new(Vec::new())),
        (Some(base), None) => base,
        (None, Some(overlay)) => overlay,
        (Some(base), Some(overlay)) => merge_preset_banks(&base, &overlay),
    };
    let presets = std::sync::Arc::new(presets);
    cache.insert(key, bank_paths.all(), presets.clone());
    Ok(presets)
}

fn cached_instrument_presets(name: &str) -> io::Result<std::sync::Arc<Vec<InstrumentPreset>>> {
    cached_instrument_presets_with_paths(crate::app_paths::app_paths(), name)
}

/// Shared, read-only view of an instrument's preset list: for factory
/// instruments this is the factory bank merged with the user's overlay bank.
/// Prefer this over [`load_instrument_presets`] wherever the list is only
/// read: a warm call is an `Arc` clone instead of a deep clone of every
/// preset's parameter maps.
pub fn load_instrument_presets_shared(
    name: &str,
) -> io::Result<std::sync::Arc<Vec<InstrumentPreset>>> {
    cached_instrument_presets(name)
}

/// Owned copy of the merged preset list. Read-only callers want
/// [`load_instrument_presets_shared`]. Callers that mutate presets and save
/// them back want [`load_user_instrument_presets`]: saving this merged list
/// would copy every factory preset into the user bank.
pub fn load_instrument_presets(name: &str) -> io::Result<Vec<InstrumentPreset>> {
    Ok(cached_instrument_presets(name)?.as_ref().clone())
}

fn load_user_instrument_presets_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<Vec<InstrumentPreset>> {
    let path = instrument_preset_save_path_with_paths(paths, name)?;
    Ok(read_preset_bank(&path)?.unwrap_or_default())
}

/// The contents of the *writable* bank only — what [`save_instrument_presets`]
/// will overwrite. For factory instruments that is the user overlay (possibly
/// empty even when the merged list is not); for user instruments it is the
/// whole bank. Load-mutate-save cycles must start from this, not from the
/// merged list.
pub fn load_user_instrument_presets(name: &str) -> io::Result<Vec<InstrumentPreset>> {
    load_user_instrument_presets_with_paths(crate::app_paths::app_paths(), name)
}

/// Drop any cached preset list that was read from `path`. Write paths that
/// bypass [`save_instrument_presets`] (the patch-fork bank materialization)
/// must call this after writing, so cache correctness does not depend on some
/// other call in the same sequence happening to invalidate the same key first.
pub fn invalidate_instrument_preset_bank_cache_at(path: &Path) {
    preset_bank_cache().lock().unwrap().invalidate_path(path);
}

/// The user overlay bank for a *factory* instrument source path, if one exists
/// on disk. `None` for user-tier sources (their bank is already writable) and
/// for factory sources with no saved user presets. Used by the patch fork so a
/// fork of a factory instrument carries the merged list the user saw, not just
/// the factory-shipped bank.
pub fn user_preset_overlay_for_factory_source(source_dsp: &Path) -> Option<PathBuf> {
    user_preset_overlay_for_factory_source_with_paths(crate::app_paths::app_paths(), source_dsp)
}

fn user_preset_overlay_for_factory_source_with_paths(
    paths: &crate::app_paths::AppPaths,
    source_dsp: &Path,
) -> Option<PathBuf> {
    if let Some(release) = source_dsp
        .parent()
        .filter(|_| source_dsp.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp"))
        .and_then(|folder| instrument_release_for_folder_with_paths(paths, folder))
    {
        // A user release's bank beside its folder is already writable.
        if release.tier != InstrumentTier::Factory {
            return None;
        }
        return Some(user_tier_preset_path(paths, &release_user_preset_logical(paths, &release)))
            .filter(|path| path.is_file());
    }
    let rel = instrument_roots(paths)
        .into_iter()
        .filter(|root| root.tier == InstrumentTier::Factory)
        .find_map(|root| source_dsp.strip_prefix(&root.path).ok().map(Path::to_path_buf))?;
    let rel = rel.as_path();
    let logical = if rel.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        rel.parent()?.to_path_buf()
    } else {
        rel.with_extension("")
    };
    let logical = logical.to_string_lossy().replace('\\', "/");
    if logical.is_empty() {
        return None;
    }
    Some(user_tier_preset_path(paths, &logical)).filter(|path| path.is_file())
}

/// Merge the user overlay bank at `overlay` into the bank file at `target`
/// (which may not exist yet), writing the merged bank back to `target`. The
/// patch fork uses this on its staged bank; `engine_name`/`source_file` are
/// placeholders the fork rewrites at finalize.
pub fn merge_preset_overlay_into_bank_file(target: &Path, overlay: &Path) -> io::Result<()> {
    let base = read_preset_bank(target)?.unwrap_or_default();
    let overlay_presets = read_preset_bank(overlay)?.unwrap_or_default();
    let bank = InstrumentPresetBank {
        version: 1,
        engine_name: String::new(),
        source_file: String::new(),
        presets: merge_preset_banks(&base, &overlay_presets),
    };
    let json = serde_json::to_string_pretty(&bank).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to serialize preset bank '{}': {e}", target.display()),
        )
    })?;
    std::fs::write(target, json)
}

pub fn load_instrument_preset_names(name: &str) -> io::Result<Vec<String>> {
    Ok(cached_instrument_presets(name)?
        .iter()
        .map(|preset| preset.name.clone())
        .collect())
}

/// Names of the presets the user saved themselves (the browser's Library
/// section): entries of the writable bank that no read-only bank (factory or
/// package) also ships. A user save over a shipped name stays with the shipped
/// preset. Every preset of a user instrument is in its writable bank, so all of
/// them are Library.
pub fn load_user_instrument_preset_names(name: &str) -> io::Result<Vec<String>> {
    load_user_instrument_preset_names_with_paths(crate::app_paths::app_paths(), name)
}

fn load_user_instrument_preset_names_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<Vec<String>> {
    let writable = instrument_preset_save_path_with_paths(paths, name)?;
    let Some(user) = read_preset_bank(&writable)? else {
        return Ok(Vec::new());
    };
    let bank_paths = instrument_preset_bank_paths_with_paths(paths, name)?;
    let mut shipped = std::collections::HashSet::new();
    for path in std::iter::once(&bank_paths.base).chain(&bank_paths.package_overlays) {
        if *path == writable {
            continue;
        }
        for preset in read_preset_bank(path)?.unwrap_or_default() {
            shipped.insert(preset.name);
        }
    }
    Ok(user
        .into_iter()
        .map(|preset| preset.name)
        .filter(|name| !shipped.contains(name))
        .collect())
}

fn preset_path_for_writable_instrument_source(source: &Path) -> PathBuf {
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source.parent().unwrap_or(source).with_extension("presets")
    } else {
        source.with_extension("presets")
    }
}

fn instrument_preset_save_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    // A versioned release saves user presets under its release name (see
    // `release_user_preset_logical`), never under the pinned id's path.
    if let Some(release) = instrument_release_with_paths(paths, name)? {
        // A user release owns its bank beside its folder (`versions/<n>.presets`
        // or `<lineage>.presets`). Presets are user data, so a retired user
        // release's bank stays writable even though its source is frozen.
        if release.tier == InstrumentTier::User {
            return Ok(release_storage_path(&release.folder, "presets"));
        }
        return Ok(user_tier_preset_path(paths, &release_user_preset_logical(paths, &release)));
    }
    match parse_instrument_id(name)? {
        Some((InstrumentTier::Factory, logical_name)) => {
            return Ok(user_tier_preset_path(paths, logical_name));
        }
        Some((InstrumentTier::Package(prefix), logical_name)) => {
            return Ok(package_tier_preset_path(paths, &prefix, logical_name));
        }
        _ => {}
    }
    let source = writable_instrument_source_path_with_paths(paths, name)?;
    Ok(preset_path_for_writable_instrument_source(&source))
}

fn save_instrument_presets_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    presets: &[InstrumentPreset],
) -> io::Result<()> {
    let path = instrument_preset_save_path_with_paths(paths, name)?;
    let key = preset_bank_cache_key(paths, name);
    let mut cache = preset_bank_cache().lock().unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The user overlay only needs to hold what differs from the factory bank.
    // Dropping entries identical to their factory counterpart keeps a later
    // factory update visible, and heals banks written before overlays existed
    // (the old single-authoritative-bank save copied the whole factory bank
    // into the user file on the first save).
    let mut presets = presets.to_vec();
    let bank_paths = instrument_preset_bank_paths_with_paths(paths, name)?;
    if bank_paths.user_overlay.as_deref() == Some(path.as_path()) {
        if let Some(factory) = read_preset_bank(&bank_paths.base)? {
            presets.retain(|preset| {
                !factory
                    .iter()
                    .any(|shipped| shipped.name == preset.name && shipped == preset)
            });
        }
    }
    let bank = InstrumentPresetBank {
        version: 1,
        engine_name: name.to_string(),
        source_file: format!("instruments/{name}.lisp"),
        presets,
    };
    let json = serde_json::to_string_pretty(&bank).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to serialize preset bank '{}': {e}", path.display()),
        )
    })?;
    std::fs::write(&path, json)?;
    cache.invalidate_key_and_path(&key, Some(&path));
    Ok(())
}

pub fn save_instrument_presets(name: &str, presets: &[InstrumentPreset]) -> io::Result<()> {
    save_instrument_presets_with_paths(crate::app_paths::app_paths(), name, presets)
}

/// Write `preset` into the factory-shipped bank beside a factory
/// instrument's source (promote to factory, eseq-jhmx), replacing a factory
/// preset of the same name in place and appending otherwise: factory banks
/// keep their authored order, whose first entry is the default. A same-name
/// entry in the user overlay is removed, since it would shadow the promoted
/// preset. Returns the bank written.
pub fn promote_instrument_preset_to_factory(
    name: &str,
    preset: &InstrumentPreset,
) -> io::Result<PathBuf> {
    promote_instrument_preset_to_factory_with_paths(crate::app_paths::app_paths(), name, preset)
}

fn promote_instrument_preset_to_factory_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    preset: &InstrumentPreset,
) -> io::Result<PathBuf> {
    // A retired release is frozen, bank included (spec §Shipping).
    if let Some(release) = instrument_release_with_paths(paths, name)? {
        if release.release != release.current {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "'{name}' is release {} of '{}', which is retired; only the current release ({}) takes factory presets",
                    release.release, release.lineage, release.current
                ),
            ));
        }
    }
    let source = resolve_instrument_storage_path_with_paths(paths, name, "lisp")?;
    let factory_root = factory_instruments_root(paths);
    let relative = source
        .strip_prefix(&factory_root)
        .ok()
        .filter(|_| source.is_file())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("'{name}' is not a factory instrument; promote the instrument first"),
            )
        })?
        .to_path_buf();
    let logical = if relative.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        format!("{}/", relative.parent().unwrap_or(&relative).to_string_lossy())
    } else {
        relative.with_extension("").to_string_lossy().to_string()
    }
    .replace('\\', "/");
    let bank_path = preset_path_for_writable_instrument_source(&source);
    let mut bank = match std::fs::read_to_string(&bank_path) {
        Ok(src) => serde_json::from_str::<InstrumentPresetBank>(&src).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse preset bank '{}': {e}", bank_path.display()),
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => InstrumentPresetBank {
            version: 1,
            engine_name: logical.clone(),
            source_file: format!("instruments/{}", relative.to_string_lossy().replace('\\', "/")),
            presets: Vec::new(),
        },
        Err(e) => return Err(e),
    };
    match bank.presets.iter_mut().find(|existing| existing.name == preset.name) {
        Some(existing) => *existing = preset.clone(),
        None => bank.presets.push(preset.clone()),
    }
    // Factory banks are written with one-space indentation; match it so a
    // promotion diffs as the preset it adds.
    let mut json = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut json, formatter);
    serde::Serialize::serialize(&bank, &mut serializer).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to serialize preset bank '{}': {e}", bank_path.display()),
        )
    })?;
    let key = preset_bank_cache_key(paths, name);
    let mut cache = preset_bank_cache().lock().unwrap();
    std::fs::write(&bank_path, json)?;
    cache.invalidate_key_and_path(&key, Some(&bank_path));

    let overlay = user_tier_preset_path(paths, logical.trim_end_matches('/'));
    if let Some(mut presets) = read_preset_bank(&overlay)? {
        let before = presets.len();
        presets.retain(|existing| existing.name != preset.name);
        if presets.len() != before {
            if presets.is_empty() {
                std::fs::remove_file(&overlay)?;
            } else {
                let overlay_bank = InstrumentPresetBank {
                    version: 1,
                    engine_name: name.to_string(),
                    source_file: format!("instruments/{name}.lisp"),
                    presets,
                };
                let json = serde_json::to_string_pretty(&overlay_bank).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to serialize preset bank '{}': {e}", overlay.display()),
                    )
                })?;
                std::fs::write(&overlay, json)?;
            }
            cache.invalidate_path(&overlay);
        }
    }
    Ok(bank_path)
}

pub(in crate::lisp_host) const INSTRUMENT_REGISTRY_SIZE: usize = MAX_INSTRUMENT_ENGINES * MAX_VOICES;
pub(in crate::lisp_host) static DGEN_INSTRUMENT_FNS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
pub(in crate::lisp_host) static DGEN_INSTRUMENT_OUTPUT_COUNTS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
/// Output channel an instrument declared `@amp true`, or `NO_AMP_CHANNEL`.
pub(in crate::lisp_host) static DGEN_INSTRUMENT_AMP_CHANNELS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(NO_AMP_CHANNEL);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
const NO_AMP_CHANNEL: usize = usize::MAX;
/// Per-voice `@amp` reading. `AMP_PENDING` until a render after the engine
/// pool armed it, then the last sample of the amp channel: on or off.
pub(in crate::lisp_host) static DGEN_VOICE_AMP_STATES: [AtomicU8; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicU8 = AtomicU8::new(AMP_PENDING);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
const AMP_OFF: u8 = 0;
const AMP_ON: u8 = 1;
const AMP_PENDING: u8 = 2;
pub(in crate::lisp_host) static DGEN_ENGINE_ENABLED_VOICES: [AtomicUsize; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
/// Voices below the enabled count that are actually retained (active or in
/// their release tail). The enabled count is a high-water mark, so one voice
/// releasing in a high slot would otherwise run every idle voice beneath it.
/// Setting a count resets this to all voices; only the engine pool narrows it.
pub(in crate::lisp_host) static DGEN_ENGINE_VOICE_MASKS: [AtomicU32; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU32 = AtomicU32::new(u32::MAX);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
pub(in crate::lisp_host) static DGEN_ENGINE_PROCESS_CALLS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
pub(in crate::lisp_host) static DGEN_ENGINE_PROCESS_BLOCKS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};

#[derive(Clone, Copy, Debug)]
pub struct DGenEngineProcessStats {
    pub engine_id: usize,
    pub enabled_voices: usize,
    pub process_calls: u64,
    pub process_blocks: u64,
}

/// Publish a voice slot's library. The fn is stored before the layout:
/// `dgenlisp_instrument_wrapper_process` reads the layout first, so any new
/// layout it sees comes with the new fn, and an old node then reads as stale.
pub fn publish_dgen_instrument_slot(
    slot_id: usize,
    process_fn: DGenProcessFn,
    output_count: usize,
    amp_channel: Option<usize>,
) {
    set_dgen_instrument_fn(slot_id, process_fn);
    set_dgen_instrument_output_count(slot_id, output_count);
    set_dgen_instrument_amp_channel(slot_id, amp_channel);
}

pub fn set_dgen_instrument_fn(slot_id: usize, f: DGenProcessFn) {
    DGEN_INSTRUMENT_FNS[slot_id % INSTRUMENT_REGISTRY_SIZE].store(f as usize, Ordering::Release);
}

pub fn set_dgen_instrument_output_count(slot_id: usize, count: usize) {
    DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
        .store(count.max(1), Ordering::Release);
}

pub fn set_dgen_instrument_amp_channel(slot_id: usize, channel: Option<usize>) {
    let slot_id = slot_id % INSTRUMENT_REGISTRY_SIZE;
    DGEN_INSTRUMENT_AMP_CHANNELS[slot_id]
        .store(channel.unwrap_or(NO_AMP_CHANNEL), Ordering::Release);
    DGEN_VOICE_AMP_STATES[slot_id].store(AMP_PENDING, Ordering::Release);
}

/// Forget any amp reading taken before now, so only a render that follows
/// this call can report the voice finished. Audio thread only.
pub fn arm_dgen_voice_amp(engine_id: usize, voice_idx: usize) {
    if engine_id < MAX_INSTRUMENT_ENGINES && voice_idx < MAX_VOICES {
        DGEN_VOICE_AMP_STATES[engine_id * MAX_VOICES + voice_idx]
            .store(AMP_PENDING, Ordering::Release);
    }
}

/// A voice's `@amp` reading since it was last armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DGenVoiceAmp {
    /// No block has rendered since the arm.
    Pending,
    /// The last rendered block ended with the amp on.
    On,
    /// The last rendered block ended with the amp off.
    Off,
}

/// `Some(reading)` for an engine whose instrument declares an `@amp`
/// output; `None` means the host cannot tell and must hold the voice.
pub fn dgen_voice_amp(engine_id: usize, voice_idx: usize) -> Option<DGenVoiceAmp> {
    if engine_id >= MAX_INSTRUMENT_ENGINES || voice_idx >= MAX_VOICES {
        return None;
    }
    let slot_id = engine_id * MAX_VOICES + voice_idx;
    if DGEN_INSTRUMENT_AMP_CHANNELS[slot_id].load(Ordering::Acquire) == NO_AMP_CHANNEL {
        return None;
    }
    Some(match DGEN_VOICE_AMP_STATES[slot_id].load(Ordering::Acquire) {
        AMP_OFF => DGenVoiceAmp::Off,
        AMP_ON => DGenVoiceAmp::On,
        _ => DGenVoiceAmp::Pending,
    })
}

pub fn set_dgen_engine_enabled_voices(engine_id: usize, count: usize) {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id].store(count.min(MAX_VOICES), Ordering::Release);
        DGEN_ENGINE_VOICE_MASKS[engine_id].store(u32::MAX, Ordering::Release);
    }
}

/// Grow the enabled count without reopening idle voices below it (the
/// engine pool's allocation path). Audio thread only.
pub fn raise_dgen_engine_enabled_voices(engine_id: usize, count: usize) {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id].store(count.min(MAX_VOICES), Ordering::Release);
    }
}

/// Mask with voices `0..count` set.
pub fn dgen_voice_bits_below(count: usize) -> u32 {
    u32::MAX.checked_shl(count as u32).map_or(u32::MAX, |above| !above)
}

/// Whether an engine voice runs this block: under the enabled count and
/// retained by the engine pool.
pub fn dgen_engine_voice_runs(engine_id: usize, voice_idx: usize) -> bool {
    engine_id >= MAX_INSTRUMENT_ENGINES
        || (voice_idx < get_dgen_engine_enabled_voices(engine_id)
            && get_dgen_engine_voice_mask(engine_id) & (1 << voice_idx) != 0)
}

/// Let `voice_idx` run again after the pool allocated it. Audio thread only.
pub fn enable_dgen_engine_voice(engine_id: usize, voice_idx: usize) {
    if engine_id < MAX_INSTRUMENT_ENGINES && voice_idx < MAX_VOICES {
        DGEN_ENGINE_VOICE_MASKS[engine_id].fetch_or(1 << voice_idx, Ordering::AcqRel);
    }
}

/// Narrow an engine's running voices to `mask` (bit = voice index) within its
/// enabled count. Audio thread only: the engine pool's retained voices.
/// Stores only on change, since every voice kernel reads this cell.
pub fn set_dgen_engine_voice_mask(engine_id: usize, mask: u32) {
    if engine_id < MAX_INSTRUMENT_ENGINES
        && DGEN_ENGINE_VOICE_MASKS[engine_id].load(Ordering::Relaxed) != mask
    {
        DGEN_ENGINE_VOICE_MASKS[engine_id].store(mask, Ordering::Release);
    }
}

pub fn get_dgen_engine_voice_mask(engine_id: usize) -> u32 {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_VOICE_MASKS[engine_id].load(Ordering::Acquire)
    } else {
        u32::MAX
    }
}

pub fn get_dgen_engine_enabled_voices(engine_id: usize) -> usize {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .min(MAX_VOICES)
    } else {
        1
    }
}

pub fn reset_dgen_engine_enabled_voices(engine_id: usize) {
    set_dgen_engine_enabled_voices(engine_id, 1);
}

pub fn take_dgen_engine_process_stats() -> Vec<DGenEngineProcessStats> {
    (0..MAX_INSTRUMENT_ENGINES)
        .map(|engine_id| DGenEngineProcessStats {
            engine_id,
            enabled_voices: get_dgen_engine_enabled_voices(engine_id),
            process_calls: DGEN_ENGINE_PROCESS_CALLS[engine_id].swap(0, Ordering::AcqRel),
            process_blocks: DGEN_ENGINE_PROCESS_BLOCKS[engine_id].swap(0, Ordering::AcqRel),
        })
        .collect()
}

/// Wrapper process function for instrument nodes. Each node runs the kernel
/// in its own header; `DGEN_INSTRUMENT_FNS` only tells whether the node is
/// still its slot's current one.
unsafe extern "C" fn dgenlisp_instrument_wrapper_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let s = state as *mut f32;
    let slot_id = (*s) as usize;
    if slot_id >= INSTRUMENT_REGISTRY_SIZE {
        return;
    }
    if (*s.add(2)).to_bits() != HEADER_CANARY.to_bits() {
        return;
    }
    // The slot's layout is read before its process fn: a rebuild stores the
    // fn first, so a new layout here implies the new fn below.
    let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id].load(Ordering::Acquire).max(1);
    let amp_channel = DGEN_INSTRUMENT_AMP_CHANNELS[slot_id].load(Ordering::Acquire);
    let current_fn = DGEN_INSTRUMENT_FNS[slot_id].load(Ordering::Acquire);
    // A node runs the kernel it was built with. A rebuild publishes the new
    // library to the slot before the old node's queued delete lands; the old
    // node's memory and ports are sized for its own library, and the slot's
    // layout globals already describe the new one, so a stale node uses none
    // of them. A header without a kernel (no build writes one) follows the slot.
    let fn_ptr = dgen_process_fn_from_state(s).map_or(current_fn, |f| f as usize);
    let stale = fn_ptr != current_fn;
    let engine_id = slot_id / MAX_VOICES;
    let voice_idx = slot_id % MAX_VOICES;
    let disabled = *s.add(DGEN_ENABLED_PARAM_IDX) <= 0.5;
    if stale && (disabled || !dgen_engine_voice_runs(engine_id, voice_idx)) {
        // Silent either way, over the ports the graph built it with.
        if !out.is_null() {
            crate::effects::silence::emit_node_outputs(out, nframes);
        }
        return;
    }
    if disabled {
        if !out.is_null() {
            // A disabled voice is silent: zero once and declare it, so the
            // voice sum and strip downstream can skip (silence propagation).
            crate::effects::silence::emit(out, output_count, nframes);
        }
        return;
    }
    if !dgen_engine_voice_runs(engine_id, voice_idx) {
        if !out.is_null() {
            // An idle voice is silent: zero once, then let downstream
            // routes skip it (silence propagation).
            crate::effects::silence::emit(out, output_count, nframes);
        }
        return;
    }
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let memory = dgen_memory_ptr(s) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        if (*out.add(0)).is_null() {
            return;
        }
        if !stale && engine_id < MAX_INSTRUMENT_ENGINES {
            DGEN_ENGINE_PROCESS_CALLS[engine_id].fetch_add(1, Ordering::Relaxed);
            if voice_idx == 0 {
                DGEN_ENGINE_PROCESS_BLOCKS[engine_id].fetch_add(1, Ordering::Relaxed);
            }
        }
        let context = dgen_process_context_v1(dgen_host_sample_rate(s));
        process_fn(
            inp as *const *const f32,
            out,
            nframes.max(0) as u32,
            memory,
            &context,
            dgen_host_services_v1(),
        );
        if stale {
            return;
        }
        record_dgen_voice_amp(slot_id, amp_channel, output_count, out, nframes);
        super::probe_capture::record_dgen_voice_probes(engine_id, voice_idx, fn_ptr, out, nframes);
    } else {
        let nf = nframes as usize;
        if !out.is_null() {
            for ch in 0..output_count {
                let out_ch = *out.add(ch);
                if !out_ch.is_null() {
                    for i in 0..nf {
                        *out_ch.add(i) = 0.0;
                    }
                }
            }
        }
    }
}

/// Latch the voice's `@amp` output at the end of the block it just rendered.
#[inline]
unsafe fn record_dgen_voice_amp(
    slot_id: usize,
    channel: usize,
    output_count: usize,
    out: *const *mut f32,
    nframes: c_int,
) {
    if channel == NO_AMP_CHANNEL || nframes <= 0 || channel >= output_count {
        return;
    }
    let amp = *out.add(channel);
    if amp.is_null() {
        return;
    }
    let state = if *amp.add(nframes as usize - 1) != 0.0 { AMP_ON } else { AMP_OFF };
    DGEN_VOICE_AMP_STATES[slot_id].store(state, Ordering::Release);
}

/// Render stand-in: what the wrapper latches after a voice block whose amp
/// channel ends on `last_amp`.
#[cfg(test)]
pub fn record_dgen_voice_amp_for_test(engine_id: usize, voice_idx: usize, last_amp: f32) {
    let slot_id = engine_id * MAX_VOICES + voice_idx;
    let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id].load(Ordering::Relaxed);
    let channel = DGEN_INSTRUMENT_AMP_CHANNELS[slot_id].load(Ordering::Relaxed);
    let mut buffers = vec![[0.0f32, last_amp]; output_count];
    let pointers: Vec<*mut f32> = buffers.iter_mut().map(|buffer| buffer.as_mut_ptr()).collect();
    unsafe { record_dgen_voice_amp(slot_id, channel, output_count, pointers.as_ptr(), 2) };
}

pub fn dgenlisp_instrument_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_instrument_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

/// Build init message for a voice-aware instrument node.
/// Sets slot_id, total_memory_slots, param defaults, tensor data,
/// and voice_cell_id = voice_index. `process_fn` is the node's own kernel
/// (see `dgenlisp_instrument_wrapper_process`).
pub fn build_init_message_for_voice(
    slot_id: usize,
    manifest: &DGenManifest,
    voice_index: usize,
    process_fn: DGenProcessFn,
) -> Vec<f32> {
    let mut entries = init_state_entries(manifest);

    // Set voice cell to voice_index
    if let Some(cell) = manifest.voice_cell_id {
        if cell < manifest.total_memory_slots {
            entries.push((cell, voice_index as f32));
        }
    }

    encode_init_message(slot_id, manifest, Some(process_fn), &entries)
}

// ── Instrument storage ──

fn writable_instrument_source_path(name: &str) -> io::Result<PathBuf> {
    writable_instrument_source_path_with_paths(crate::app_paths::app_paths(), name)
}

fn writable_instrument_source_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    let logical_name = match parse_instrument_id(name)? {
        Some((InstrumentTier::Factory, _)) => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("factory instrument '{name}' is read-only; fork it before editing"),
            ));
        }
        Some((InstrumentTier::Package(_), _)) => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("package instrument '{name}' is read-only; fork it before editing"),
            ));
        }
        Some((InstrumentTier::User, logical_name)) => {
            // A retired user release is frozen (patcher-workspace spec §5):
            // only the lineage's current release takes source writes.
            if let Some(release) = instrument_release_with_paths(paths, name)? {
                if release.release != release.current {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "'{name}' is release {} of '{}', which is frozen; save to the current release (user:{}@{})",
                            release.release, release.lineage, release.lineage, release.current
                        ),
                    ));
                }
            }
            let existing = resolve_instrument_storage_path_with_paths(paths, name, "lisp")?;
            // Only an existing file inside the user tier is a write target; a
            // resolution that landed anywhere else (factory tree, dev fixture
            // root) must never be handed to `fs::write`.
            if existing.is_file() && existing.starts_with(paths.user_instruments_dir()) {
                return Ok(existing);
            }
            logical_name
        }
        None => name.trim_end_matches('/'),
    };
    let root = paths.user_instruments_dir();
    if name.ends_with('/') {
        Ok(root.join(logical_name).join("dsp.lisp"))
    } else {
        Ok(root.join(format!("{logical_name}.lisp")))
    }
}

fn save_instrument_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    source: &str,
) -> io::Result<()> {
    let path = writable_instrument_source_path_with_paths(paths, name)?;
    let preset_path = preset_path_for_writable_instrument_source(&path);
    let key = preset_bank_cache_key(paths, name);
    let mut cache = preset_bank_cache().lock().unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)?;
    cache.invalidate_key_and_path(&key, Some(&preset_path));
    Ok(())
}

pub fn save_instrument(name: &str, source: &str) -> io::Result<()> {
    save_instrument_with_paths(crate::app_paths::app_paths(), name, source)
}

// ── User-tier release cutting (docs/patcher-workspace-spec.md §5) ──

/// What [`cut_user_release`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserReleaseCut {
    /// Logical path of the lineage's top folder under the user tier.
    pub lineage: String,
    /// The release that was frozen into `versions/<frozen>/`.
    pub frozen: u32,
    /// The new working release, now `current`, living at the top folder.
    pub release: u32,
    /// `user:<lineage>@<release>`: the id to bind the edited track to, and
    /// the name to save the new source through (an unpinned `user:<lineage>`
    /// now resolves to release 1, which is frozen).
    pub id: String,
    /// The top folder's `dsp.lisp`, where the new release's source goes. A
    /// legacy single-file instrument's path changes here (`<name>.lisp` →
    /// `<name>/dsp.lisp`).
    pub source_path: PathBuf,
}

/// Freeze the current release of the user instrument whose current-release
/// source is `source_path` and open the next one (patcher-workspace spec
/// §5.1; the factory recipe in instrument-versioning spec §Shipping).
///
/// - Every top-level file of the instrument folder except `instrument.json`
///   (`dsp.lisp`, `ui.lisp`, `dsp.layout.json`, assets) is copied into
///   `versions/<current>/`, and the bank `<folder>.presets` into
///   `versions/<current>.presets`. The top-level copies stay, so the new
///   release starts from the frozen one (its presets included) until the
///   caller writes the new source to [`UserReleaseCut::source_path`].
/// - The frozen folder gets its own `instrument.json` (run mode and voice
///   controls; no manifest fields), since a release inherits nothing.
/// - The manifest points `releases[current]` at `versions/<current>`, adds
///   `releases[current + 1] = { path: ".", name }` and bumps `current`.
///   `name` is `new_name` when given (a rename), otherwise the name the
///   frozen release carried. A rename does not move the top folder in this
///   rev: the lineage (and so every pinned id) keeps the folder's path, and
///   the unpinned new name finds the release through the name-match rule.
/// - An unversioned instrument first becomes versioned: its files are
///   release 1, named after its logical path.
/// - A legacy single-file instrument (`<name>.lisp`, `<name>.instrument.json`,
///   `<name>.layout.json`) first moves into folder form (`<name>/dsp.lisp`,
///   `<name>/instrument.json`, `<name>/dsp.layout.json`); its bank
///   `<name>.presets` is already where the folder's bank lives. An unpinned
///   `user:<name>` then resolves to release 1.
///
/// Nothing under `versions/` is modified once written: a cut whose target
/// release folder or bank already exists fails. Caches (release index,
/// resolved paths, preset banks) are invalidated before returning.
pub fn cut_user_release(source_path: &Path, new_name: Option<&str>) -> Result<UserReleaseCut, String> {
    cut_user_release_with_paths(crate::app_paths::app_paths(), source_path, new_name)
}

fn cut_user_release_with_paths(
    paths: &crate::app_paths::AppPaths,
    source_path: &Path,
    new_name: Option<&str>,
) -> Result<UserReleaseCut, String> {
    let user_root = paths.user_instruments_dir();
    let display = source_path.display();
    if !source_path.is_file() {
        return Err(format!("instrument source '{display}' does not exist"));
    }
    let relative = source_path
        .strip_prefix(&user_root)
        .map_err(|_| format!("'{display}' is not a user-tier instrument; only user instruments cut releases"))?;
    let new_name = match new_name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => Some(
            validate_instrument_relative_dir(name)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/"),
        ),
        None => None,
    };
    let folder = if source_path.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        let folder = source_path.parent().expect("dsp.lisp has a parent").to_path_buf();
        if folder.parent().and_then(|parent| parent.file_name()).and_then(|name| name.to_str())
            == Some("versions")
            && folder.parent().and_then(Path::parent).is_some_and(|top| top.join("instrument.json").is_file())
        {
            return Err(format!("'{display}' is a frozen release; cut from the lineage's top folder"));
        }
        folder
    } else {
        migrate_legacy_user_instrument(source_path)?
    };
    let lineage = folder
        .strip_prefix(&user_root)
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    if lineage.is_empty() {
        return Err(format!("'{}' is not an instrument source", relative.display()));
    }

    let manifest_path = folder.join("instrument.json");
    let mut metadata = match std::fs::read_to_string(&manifest_path) {
        Ok(source) => serde_json::from_str::<InstrumentMetadataFile>(&source)
            .map_err(|error| format!("invalid instrument metadata '{}': {error}", manifest_path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => InstrumentMetadataFile {
            version: 1,
            run_mode: CustomInstrumentRunMode::Instrument.as_str().to_string(),
            voice_controls: None,
            current: None,
            releases: None,
            deprecated: false,
            replaced_by: None,
        },
        Err(error) => return Err(format!("cannot read '{}': {error}", manifest_path.display())),
    };
    let mut releases = metadata.releases.take().unwrap_or_default();
    if releases.is_empty() {
        releases.insert(1, InstrumentReleaseEntry { path: ".".to_string(), name: lineage.clone() });
    }
    let frozen = metadata
        .current
        .unwrap_or_else(|| *releases.keys().next_back().expect("non-empty releases"));
    let frozen_entry = releases
        .get(&frozen)
        .cloned()
        .ok_or_else(|| format!("current release {frozen} of '{lineage}' is not listed in releases"))?;
    if frozen_entry.path.trim_end_matches('/') != "." {
        return Err(format!(
            "current release {frozen} of '{lineage}' lives at '{}', not the top folder",
            frozen_entry.path
        ));
    }
    let release = releases.keys().next_back().expect("non-empty releases") + 1;
    let release_name = new_name.unwrap_or_else(|| frozen_entry.name.clone());
    if release_name != lineage {
        if let Some(other) = user_instrument_owning_name(paths, &release_name, &folder) {
            return Err(format!("cannot name release {release} of '{lineage}' '{release_name}': {other}"));
        }
    }

    // Freeze into a staging folder, then rename it into place, so a failed
    // copy never leaves a half-written `versions/<n>`.
    let versions = folder.join("versions");
    let target = versions.join(frozen.to_string());
    let target_bank = versions.join(format!("{frozen}.presets"));
    for path in [&target, &target_bank] {
        if path.exists() {
            return Err(format!("frozen release '{}' already exists", path.display()));
        }
    }
    let staging = versions.join(format!(".{frozen}.staging"));
    let io_error = |path: &Path, error: io::Error| format!("'{}': {error}", path.display());
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| io_error(&staging, error))?;
    }
    std::fs::create_dir_all(&staging).map_err(|error| io_error(&staging, error))?;
    let entries = std::fs::read_dir(&folder).map_err(|error| io_error(&folder, error))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue };
        if !path.is_file() || file_name == "instrument.json" || file_name.starts_with('.') {
            continue;
        }
        std::fs::copy(&path, staging.join(file_name)).map_err(|error| io_error(&path, error))?;
    }
    if manifest_path.is_file() {
        let release_metadata = InstrumentMetadataFile {
            version: metadata.version,
            run_mode: metadata.run_mode.clone(),
            voice_controls: metadata.voice_controls.clone(),
            current: None,
            releases: None,
            deprecated: false,
            replaced_by: None,
        };
        write_instrument_metadata(&staging.join("instrument.json"), &release_metadata)?;
    }
    let bank = release_storage_path(&folder, "presets");
    let staging_bank = versions.join(format!(".{frozen}.presets.staging"));
    if bank.is_file() {
        std::fs::copy(&bank, &staging_bank).map_err(|error| io_error(&bank, error))?;
    }
    std::fs::rename(&staging, &target).map_err(|error| io_error(&target, error))?;
    if bank.is_file() {
        std::fs::rename(&staging_bank, &target_bank).map_err(|error| io_error(&target_bank, error))?;
    }

    releases.insert(
        frozen,
        InstrumentReleaseEntry { path: format!("versions/{frozen}"), name: frozen_entry.name },
    );
    releases.insert(release, InstrumentReleaseEntry { path: ".".to_string(), name: release_name });
    metadata.releases = Some(releases);
    metadata.current = Some(release);
    write_instrument_metadata(&manifest_path, &metadata)?;

    invalidate_instrument_release_index();
    {
        // Drop lists merged from the old bank: an unpinned `user:<lineage>`
        // now reads release 1's bank. (`invalidate_path` on a path nothing
        // read would re-index every instrument for no reason.)
        let mut cache = preset_bank_cache().lock().unwrap();
        for path in [&bank, &target_bank] {
            if cache.entries.values().any(|entry| entry.paths.iter().any(|p| p == path)) {
                cache.invalidate_path(path);
            }
        }
    }
    Ok(UserReleaseCut {
        id: InstrumentTier::User.qualify(&format!("{lineage}@{release}")),
        lineage,
        frozen,
        release,
        source_path: folder.join("dsp.lisp"),
    })
}

/// Why `name` cannot become a release name of the lineage at `own_folder`:
/// another user instrument already answers to it. `None` when it is free.
fn user_instrument_owning_name(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    own_folder: &Path,
) -> Option<String> {
    let root = paths.user_instruments_dir();
    let folder = root.join(name);
    if folder != own_folder && (folder.join("dsp.lisp").is_file() || root.join(format!("{name}.lisp")).is_file()) {
        return Some(format!("user instrument '{name}' already exists"));
    }
    instrument_release_index(paths)
        .iter()
        .filter(|instrument| instrument.tier == InstrumentTier::User)
        .filter(|instrument| root.join(&instrument.lineage) != own_folder)
        .find(|instrument| instrument.releases.values().any(|(_, release_name)| release_name == name))
        .map(|instrument| format!("a release of '{}' already ships under that name", instrument.lineage))
}

/// Move a legacy single-file user instrument into folder form and return
/// the folder. The bank (`<name>.presets`) already sits where the folder's
/// bank lives; a legacy `<name>/ui.lisp` is already inside the folder.
fn migrate_legacy_user_instrument(source: &Path) -> Result<PathBuf, String> {
    if source.extension().and_then(|ext| ext.to_str()) != Some("lisp") {
        return Err(format!("'{}' is not an instrument source", source.display()));
    }
    let folder = source.with_extension("");
    let dsp = folder.join("dsp.lisp");
    if dsp.exists() {
        return Err(format!(
            "cannot migrate '{}' to folder form: '{}' already exists",
            source.display(),
            dsp.display()
        ));
    }
    let moves = [
        (source.with_extension("instrument.json"), folder.join("instrument.json")),
        (source.with_extension("layout.json"), folder.join("dsp.layout.json")),
    ];
    for (from, to) in &moves {
        if from.is_file() && to.exists() {
            return Err(format!(
                "cannot migrate '{}' to folder form: '{}' already exists",
                from.display(),
                to.display()
            ));
        }
    }
    std::fs::create_dir_all(&folder).map_err(|error| format!("'{}': {error}", folder.display()))?;
    std::fs::rename(source, &dsp).map_err(|error| format!("'{}': {error}", source.display()))?;
    for (from, to) in moves {
        if from.is_file() {
            std::fs::rename(&from, &to).map_err(|error| format!("'{}': {error}", from.display()))?;
        }
    }
    invalidate_instrument_release_index();
    Ok(folder)
}

fn write_instrument_metadata(path: &Path, metadata: &InstrumentMetadataFile) -> Result<(), String> {
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|error| format!("failed to encode instrument metadata: {error}"))?;
    // Write beside and rename, so a crash never leaves a truncated manifest.
    let staging = path.with_extension("json.tmp");
    std::fs::write(&staging, format!("{json}\n")).map_err(|error| format!("'{}': {error}", staging.display()))?;
    std::fs::rename(&staging, path).map_err(|error| format!("'{}': {error}", path.display()))
}

pub fn save_instrument_ui(name: &str, source: &str) -> io::Result<()> {
    let instrument_source = writable_instrument_source_path(name)?;
    let path = if instrument_source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        instrument_source.parent().unwrap_or(&instrument_source).join("ui.lisp")
    } else {
        instrument_source.with_extension("").join("ui.lisp")
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)
}

pub fn instrument_ui_path(name: &str) -> io::Result<PathBuf> {
    if name.ends_with('/') {
        let direct = instrument_source_roots()
            .into_iter()
            .map(|root| root.join(name.trim_end_matches('/')).join("ui.lisp"))
            .find(|path| path.exists());
        if let Some(direct) = direct {
            return Ok(direct);
        }
    } else {
        if let Some(direct) = instrument_source_roots()
            .into_iter()
            .map(|root| root.join(name).join("ui.lisp"))
            .find(|path| path.exists())
        {
            return Ok(direct);
        }
    }
    Ok(resolve_instrument_folder_path(name)?.join("ui.lisp"))
}

pub fn load_instrument_ui_source(name: &str) -> io::Result<String> {
    std::fs::read_to_string(instrument_ui_path(name)?)
}

pub fn list_saved_instruments() -> Vec<String> {
    list_saved_instruments_in(crate::app_paths::app_paths().instrument_dirs())
}

fn list_saved_instruments_in(dirs: Vec<PathBuf>) -> Vec<String> {
    fn is_hidden(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }

    fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                // Frozen releases of a versioned instrument
                // (`<instrument>/versions/<n>`) are reached through the
                // instrument's pinned id, never listed as instruments.
                if dir.join("dsp.lisp").exists()
                    && path.file_name().and_then(|name| name.to_str()) == Some("versions")
                {
                    continue;
                }
                if path.join("dsp.lisp").exists() {
                    // Deprecated instruments stay loadable by id but are
                    // no longer offered (spec §Deprecation).
                    if instrument_metadata_is_deprecated(&path.join("instrument.json")) {
                        continue;
                    }
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(format!("{}/", rel.to_string_lossy().replace('\\', "/")));
                    }
                }
                collect(&path, root, out);
            } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                let file_stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                if matches!(file_stem, "dsp" | "ui" | "presets") {
                    continue;
                }
                if instrument_source_is_deprecated(&path) {
                    continue;
                }
                if let Ok(rel) = path.strip_prefix(root) {
                    let without_ext = rel.with_extension("");
                    out.push(without_ext.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }

    let mut names = Vec::new();
    for dir in dirs {
        collect(&dir, &dir, &mut names);
    }
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

pub(in crate::lisp_host) fn validate_instrument_relative_dir(path: &str) -> io::Result<PathBuf> {
    let trimmed = path.trim().trim_matches('/');
    let mut relative = PathBuf::new();
    if trimmed.is_empty() {
        return Ok(relative);
    }
    for component in Path::new(trimmed).components() {
        match component {
            std::path::Component::Normal(part) => relative.push(part),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid instrument folder '{path}'"),
                ));
            }
        }
    }
    Ok(relative)
}

pub fn move_saved_instrument(name: &str, target_folder: &str) -> io::Result<String> {
    move_saved_instrument_with_paths(crate::app_paths::app_paths(), name, target_folder)
}

fn move_saved_instrument_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    target_folder: &str,
) -> io::Result<String> {
    let root = paths.user_instruments_dir();
    let source = writable_instrument_source_path_with_paths(paths, name)?;
    if !source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("instrument '{name}' does not exist"),
        ));
    }
    // A versioned instrument's top folder is its lineage: every `@n` pin a
    // saved project holds spells that path, so moving it would orphan them
    // (patcher-workspace spec §5).
    if let Some(release) = source
        .parent()
        .filter(|_| source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp"))
        .and_then(|folder| instrument_release_for_folder_with_paths(paths, folder))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "cannot move '{}': it has saved releases, and projects pin them by its path ({}@<n>)",
                release.lineage,
                release.tier.qualify(&release.lineage)
            ),
        ));
    }

    let key = preset_bank_cache_key(paths, name);
    let preset_path = preset_path_for_writable_instrument_source(&source);
    preset_bank_cache()
        .lock()
        .unwrap()
        .invalidate_key_and_path(&key, Some(&preset_path));

    let target_dir = root.join(validate_instrument_relative_dir(target_folder)?);
    if !target_dir.exists() || !target_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "target instrument folder '{}' does not exist",
                target_dir.display()
            ),
        ));
    }

    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        let source_dir = source.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "instrument source '{}' has no parent directory",
                    source.display()
                ),
            )
        })?;
        if source_dir == target_dir {
            return Ok(name.to_string());
        }
        if target_dir.starts_with(source_dir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot move an instrument folder into itself",
            ));
        }
        let folder_name = source_dir.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument folder '{}' has no name", source_dir.display()),
            )
        })?;
        let dest = target_dir.join(folder_name);
        if dest.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("target instrument '{}' already exists", dest.display()),
            ));
        }
        std::fs::rename(source_dir, &dest)?;
        return dest
            .strip_prefix(root)
            .map(|rel| format!("{}/", rel.to_string_lossy().replace('\\', "/")))
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()));
    }

    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument source '{}' has no file stem", source.display()),
            )
        })?;
    let dest_source = target_dir.join(format!("{stem}.lisp"));
    if dest_source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "target instrument '{}' already exists",
                dest_source.display()
            ),
        ));
    }
    let mut sidecars = Vec::new();
    for extension in ["presets", "instrument.json"] {
        let sidecar = source.with_extension(extension);
        if sidecar.exists() {
            let dest = target_dir.join(sidecar.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("sidecar '{}' has no file name", sidecar.display()),
                )
            })?);
            if dest.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("target sidecar '{}' already exists", dest.display()),
                ));
            }
            sidecars.push((sidecar, dest));
        }
    }
    std::fs::rename(&source, &dest_source)?;
    for (sidecar, dest) in sidecars {
        std::fs::rename(sidecar, dest)?;
    }
    dest_source
        .strip_prefix(root)
        .map(|rel| rel.with_extension("").to_string_lossy().replace('\\', "/"))
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))
}

pub fn load_instrument_source(name: &str) -> io::Result<String> {
    let path = resolve_instrument_storage_path(name, "lisp")?;
    std::fs::read_to_string(&path)
}

#[cfg(test)]
mod tier_id_tests {
    use super::*;

    #[test]
    fn promoting_a_preset_writes_the_factory_bank_and_clears_the_overlay_copy() {
        let (paths, root) = test_paths("promote");
        let factory = paths.instruments_dir();
        write_folder_instrument(&factory, "Drums/Kick", "(factory)");
        write_preset_bank(&factory.join("Drums/Kick.presets"), "Drums/Kick/", &["Default", "Old"]);
        let overlay = paths.user_instruments_dir().join("Drums/Kick.presets");
        std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        write_preset_bank(&overlay, "factory:Drums/Kick", &["Punchy", "Mine"]);

        let mut punchy = preset("Punchy");
        punchy.params.insert("decay".into(), 0.25);
        let bank = promote_instrument_preset_to_factory_with_paths(
            &paths,
            "factory:Drums/Kick",
            &punchy,
        )
        .unwrap();
        assert_eq!(bank, factory.join("Drums/Kick.presets"));
        let written: InstrumentPresetBank =
            serde_json::from_str(&std::fs::read_to_string(&bank).unwrap()).unwrap();
        let names: Vec<_> = written.presets.iter().map(|p| p.name.as_str()).collect();
        // Authored order is kept; the default stays first.
        assert_eq!(names, ["Default", "Old", "Punchy"]);
        assert_eq!(written.presets[2].params.get("decay"), Some(&0.25));
        let overlay_left = read_preset_bank(&overlay).unwrap().unwrap();
        assert_eq!(overlay_left.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Mine"]);

        // Re-promoting replaces in place.
        punchy.params.insert("decay".into(), 0.5);
        promote_instrument_preset_to_factory_with_paths(&paths, "factory:Drums/Kick", &punchy)
            .unwrap();
        let written: InstrumentPresetBank =
            serde_json::from_str(&std::fs::read_to_string(&bank).unwrap()).unwrap();
        assert_eq!(written.presets.len(), 3);
        assert_eq!(written.presets[2].params.get("decay"), Some(&0.5));

        // A user-tier instrument has no factory bank to write.
        write_folder_instrument(&paths.user_instruments_dir(), "Mine/Synth", "(user)");
        assert!(promote_instrument_preset_to_factory_with_paths(&paths, "user:Mine/Synth", &punchy)
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn test_paths(label: &str) -> (crate::app_paths::AppPaths, PathBuf) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("eseq-instrument-id-{label}-{unique}"));
        let paths = crate::app_paths::AppPaths::dev(
            root.join("crates/sequencer"),
            root.clone(),
            root.join("config"),
        );
        (paths, root)
    }

    fn write_folder_instrument(root: &Path, name: &str, marker: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dsp.lisp"), marker).unwrap();
    }

    fn preset(name: &str) -> InstrumentPreset {
        InstrumentPreset {
            id: name.to_lowercase(),
            name: name.to_string(),
            base_note_offset: 0.0,
            params: std::collections::BTreeMap::new(),
            key_locks: std::collections::BTreeMap::new(),
        }
    }

    fn write_preset_bank(path: &Path, instrument_name: &str, names: &[&str]) {
        let bank = InstrumentPresetBank {
            version: 1,
            engine_name: instrument_name.to_string(),
            source_file: format!("instruments/{instrument_name}.lisp"),
            presets: names.iter().map(|name| preset(name)).collect(),
        };
        std::fs::write(path, serde_json::to_string_pretty(&bank).unwrap()).unwrap();
    }

    fn names(presets: &[InstrumentPreset]) -> Vec<&str> {
        presets.iter().map(|preset| preset.name.as_str()).collect()
    }

    /// A manifest-backed package under the user's packages dir carrying one
    /// folder instrument. Content-only: no `src/`, no entry module.
    fn write_package_instrument(
        paths: &crate::app_paths::AppPaths,
        identity: &str,
        name: &str,
        marker: &str,
    ) -> PathBuf {
        let package = paths.packages_dir().join(identity.replace('/', "."));
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("manifest.json"),
            format!(r#"{{"name":"{identity}","version":"1"}}"#),
        )
        .unwrap();
        let instruments = package.join("instruments");
        write_folder_instrument(&instruments, name, marker);
        crate::app_paths::invalidate_package_catalog_cache();
        instruments
    }

    #[test]
    fn package_ids_resolve_only_inside_their_package() {
        let (paths, root) = test_paths("package-ids");
        write_folder_instrument(&paths.instruments_dir(), "kick", "factory");
        write_folder_instrument(&paths.user_instruments_dir(), "kick", "user");
        let package_root = write_package_instrument(&paths, "alec/drums", "kick", "package");

        let resolve = |name: &str| resolve_instrument_storage_path_with_paths(&paths, name, "lisp").unwrap();
        assert_eq!(resolve("pkg:alec.drums/kick"), package_root.join("kick/dsp.lisp"));
        assert_eq!(resolve("pkg:alec.drums/kick/"), package_root.join("kick/dsp.lisp"));
        assert_eq!(resolve("factory:kick"), paths.instruments_dir().join("kick/dsp.lisp"));
        assert_eq!(resolve("user:kick"), paths.user_instruments_dir().join("kick/dsp.lisp"));
        // Legacy bare names keep resolving factory-first: a package never
        // shadows the shipped tree or the user's library.
        assert_eq!(resolve("kick"), paths.instruments_dir().join("kick/dsp.lisp"));

        // The id round-trips through the source path.
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "pkg:alec.drums/kick").unwrap(),
            "pkg:alec.drums/kick"
        );
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "pkg:alec.drums/kick/").unwrap(),
            "pkg:alec.drums/kick"
        );

        // Missing inside the package: stays inside the package, never borrows
        // the same-named factory or user instrument.
        let missing = resolve("pkg:alec.drums/snare");
        assert!(missing.starts_with(&package_root), "{}", missing.display());
        assert!(!missing.exists());
        assert!(qualify_instrument_id_with_paths(&paths, "pkg:alec.drums/snare").is_err());

        // An id from a package that is not installed is an error, not a
        // silent fall-through.
        let error = resolve_instrument_storage_path_with_paths(&paths, "pkg:nobody.pack/kick", "lisp")
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        // A malformed package id is rejected up front.
        assert_eq!(
            resolve_instrument_storage_path_with_paths(&paths, "pkg:alec.drums", "lisp")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bare_names_reach_a_package_only_when_no_tier_has_them() {
        let (paths, root) = test_paths("package-bare");
        let package_root = write_package_instrument(&paths, "alec/drums", "snare", "package");
        assert_eq!(
            resolve_instrument_storage_path_with_paths(&paths, "snare", "lisp").unwrap(),
            package_root.join("snare/dsp.lisp")
        );
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "snare").unwrap(),
            "pkg:alec.drums/snare",
            "a bare name that lands in a package qualifies as that package's"
        );
        write_folder_instrument(&paths.user_instruments_dir(), "snare", "user");
        // The walk cache revalidates by existence only, so the new user copy is
        // not observed until the cached package path goes away; a fresh
        // resolution of the same name on a fresh name string is the contract.
        resolved_walk_cache().lock().unwrap().clear();
        assert_eq!(
            resolve_instrument_storage_path_with_paths(&paths, "snare", "lisp").unwrap(),
            paths.user_instruments_dir().join("snare/dsp.lisp")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_instruments_are_read_only_with_a_user_preset_overlay() {
        let (paths, root) = test_paths("package-presets");
        let package_root = write_package_instrument(&paths, "alec/drums", "kick", "package");
        write_preset_bank(&package_root.join("kick.presets"), "pkg:alec.drums/kick", &["Tight"]);
        let name = "pkg:alec.drums/kick/";

        let error = writable_instrument_source_path_with_paths(&paths, name).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(save_instrument_with_paths(&paths, name, "edited").is_err());
        assert_eq!(
            std::fs::read_to_string(package_root.join("kick/dsp.lisp")).unwrap(),
            "package"
        );

        // Presets save into a per-package overlay under the user tier that no
        // instrument walk lists, and load merged with the shipped bank.
        let overlay = instrument_preset_save_path_with_paths(&paths, name).unwrap();
        assert_eq!(
            overlay,
            paths
                .user_instruments_dir()
                .join(".package-presets/alec.drums/kick.presets")
        );
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, name).unwrap()),
            vec!["Tight"]
        );
        save_instrument_presets_with_paths(&paths, name, &[preset("Loose"), preset("Tight")]).unwrap();
        assert!(overlay.is_file());
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, name).unwrap()),
            vec!["Loose", "Tight"]
        );
        // The overlay directory is hidden from the bare-name walk.
        assert!(resolve_instrument_storage_path_with_paths(&paths, "kick", "lisp")
            .unwrap()
            .starts_with(&package_root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_preset_banks_overlay_factory_instruments_under_the_user_bank() {
        let (paths, root) = test_paths("package-preset-overlay");
        std::fs::create_dir_all(paths.instruments_dir()).unwrap();
        write_folder_instrument(&paths.instruments_dir(), "heat", "factory");
        write_preset_bank(&paths.instruments_dir().join("heat.presets"), "factory:heat", &["Init"]);
        let package = paths.packages_dir().join("alec.heatpack");
        std::fs::create_dir_all(package.join("presets/factory")).unwrap();
        std::fs::write(package.join("manifest.json"), r#"{"name":"alec/heatpack","version":"1"}"#).unwrap();
        write_preset_bank(
            &package.join("presets/factory/heat.presets"),
            "factory:heat",
            &["Init", "Pack Lead"],
        );
        crate::app_paths::invalidate_package_catalog_cache();

        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, "factory:heat").unwrap()),
            vec!["Init", "Pack Lead"]
        );
        // Bare names see the pack too (they resolve factory-first).
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, "heat").unwrap()),
            vec!["Init", "Pack Lead"]
        );
        // The user's own save still wins by name and adds on top.
        save_instrument_presets_with_paths(
            &paths,
            "factory:heat",
            &[preset("Mine"), preset("Pack Lead")],
        )
        .unwrap();
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, "factory:heat").unwrap()),
            vec!["Init", "Mine", "Pack Lead"]
        );
        // A package instrument has no overlay slot: its bank travels with it.
        assert_eq!(package_preset_overlay_relative("pkg:alec.heatpack/x").unwrap(), None);
        assert_eq!(
            package_preset_overlay_relative("user:kits/kick").unwrap(),
            Some(PathBuf::from("user/kits/kick.presets"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_preset_names_are_the_writable_bank_minus_shipped_names() {
        let (paths, root) = test_paths("user-preset-names");
        std::fs::create_dir_all(paths.instruments_dir()).unwrap();
        write_folder_instrument(&paths.instruments_dir(), "split", "factory");
        write_preset_bank(
            &paths.instruments_dir().join("split.presets"),
            "factory:split",
            &["Bright", "Dark"],
        );
        let overlay = paths.user_instruments_dir().join("split.presets");
        std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        // "Dark" is a user edit of a shipped preset: it stays Factory.
        write_preset_bank(&overlay, "factory:split", &["Dark", "Mine"]);
        assert_eq!(
            load_user_instrument_preset_names_with_paths(&paths, "factory:split").unwrap(),
            vec!["Mine"]
        );

        // A user instrument's whole bank is the user's.
        write_folder_instrument(&paths.user_instruments_dir(), "Own", "user");
        write_preset_bank(&paths.user_instruments_dir().join("Own.presets"), "user:Own", &["A", "B"]);
        assert_eq!(
            load_user_instrument_preset_names_with_paths(&paths, "user:Own").unwrap(),
            vec!["A", "B"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn factory_and_user_banks_merge_with_user_presets_shadowing_by_name() {
        let (paths, root) = test_paths("factory-user-merge");
        let instrument_name = "factory:merged";
        std::fs::create_dir_all(paths.instruments_dir()).unwrap();
        write_folder_instrument(&paths.instruments_dir(), "merged", "factory");
        write_preset_bank(
            &paths.instruments_dir().join("merged.presets"),
            instrument_name,
            &["Bright", "Dark"],
        );

        // No user bank yet: the factory-shipped bank alone.
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, instrument_name).unwrap()),
            vec!["Bright", "Dark"]
        );
        assert!(load_user_instrument_presets_with_paths(&paths, instrument_name)
            .unwrap()
            .is_empty());

        // Saving routes to the user tier and only that bank is written; the
        // read side is the union, sorted by name.
        save_instrument_presets_with_paths(&paths, instrument_name, &[preset("Custom")]).unwrap();
        let user_bank = paths.user_instruments_dir().join("merged.presets");
        assert!(user_bank.is_file());
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, instrument_name).unwrap()),
            vec!["Bright", "Custom", "Dark"],
            "the merged list must show factory and user presets together"
        );
        assert_eq!(
            names(&load_user_instrument_presets_with_paths(&paths, instrument_name).unwrap()),
            vec!["Custom"],
            "the writable bank holds only the user's presets"
        );

        // A user preset with a factory name shadows the factory one.
        let mut shadow = preset("Dark");
        shadow.base_note_offset = 12.0;
        save_instrument_presets_with_paths(
            &paths,
            instrument_name,
            &[preset("Custom"), shadow.clone()],
        )
        .unwrap();
        let merged = cached_instrument_presets_with_paths(&paths, instrument_name).unwrap();
        assert_eq!(names(&merged), vec!["Bright", "Custom", "Dark"]);
        let dark = merged.iter().find(|p| p.name == "Dark").unwrap();
        assert_eq!(dark.base_note_offset, 12.0, "user copy must win over the factory one");

        // Factory presets are not deletable: a user bank that omits them does
        // not remove them from the merged list.
        save_instrument_presets_with_paths(&paths, instrument_name, &[]).unwrap();
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, instrument_name).unwrap()),
            vec!["Bright", "Dark"]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_bare_name_for_a_factory_instrument_merges_its_user_tier_saves() {
        // Engine registries hand out bare ids (`factory/digiwave/`), not
        // `factory:`-qualified ones. Those resolve factory-first on read but
        // save into the user tier; the saved presets must still be visible.
        let (paths, root) = test_paths("bare-name-overlay");
        std::fs::create_dir_all(paths.instruments_dir().join("factory")).unwrap();
        write_folder_instrument(&paths.instruments_dir(), "factory/bare", "factory");
        write_preset_bank(
            &paths.instruments_dir().join("factory/bare.presets"),
            "factory/bare/",
            &["init"],
        );
        let bare = "factory/bare/";
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, bare).unwrap()),
            vec!["init"]
        );

        save_instrument_presets_with_paths(&paths, bare, &[preset("testsave")]).unwrap();
        assert!(paths
            .user_instruments_dir()
            .join("factory/bare.presets")
            .is_file());
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, bare).unwrap()),
            vec!["init", "testsave"],
            "a preset saved under a bare factory name must show up on the next load"
        );
        assert_eq!(
            names(&load_user_instrument_presets_with_paths(&paths, bare).unwrap()),
            vec!["testsave"]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saving_prunes_user_presets_identical_to_factory_ones() {
        let (paths, root) = test_paths("prune-factory-copies");
        let instrument_name = "factory:pruned";
        std::fs::create_dir_all(paths.instruments_dir()).unwrap();
        write_folder_instrument(&paths.instruments_dir(), "pruned", "factory");
        write_preset_bank(
            &paths.instruments_dir().join("pruned.presets"),
            instrument_name,
            &["Init"],
        );

        // The old single-authoritative-bank save copied the factory bank into
        // the user file; saving that shape again heals it.
        let mut changed = preset("Init");
        changed.base_note_offset = 5.0;
        save_instrument_presets_with_paths(
            &paths,
            instrument_name,
            &[preset("Init"), preset("Mine")],
        )
        .unwrap();
        assert_eq!(
            names(&load_user_instrument_presets_with_paths(&paths, instrument_name).unwrap()),
            vec!["Mine"],
            "a user preset identical to the factory one is not stored"
        );

        // A genuinely different preset of the same name is kept as a shadow.
        save_instrument_presets_with_paths(&paths, instrument_name, &[changed, preset("Mine")])
            .unwrap();
        assert_eq!(
            names(&load_user_instrument_presets_with_paths(&paths, instrument_name).unwrap()),
            vec!["Init", "Mine"]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_fork_of_a_factory_instrument_carries_the_user_preset_overlay() {
        let (paths, root) = test_paths("fork-overlay");
        std::fs::create_dir_all(paths.instruments_dir()).unwrap();
        write_folder_instrument(&paths.instruments_dir(), "core/forked", "factory");
        let source_dsp = paths.instruments_dir().join("core/forked/dsp.lisp");
        assert_eq!(
            user_preset_overlay_for_factory_source_with_paths(&paths, &source_dsp),
            None,
            "no user bank, no overlay"
        );
        save_instrument_presets_with_paths(&paths, "factory:core/forked", &[preset("Mine")])
            .unwrap();
        let overlay = user_preset_overlay_for_factory_source_with_paths(&paths, &source_dsp)
            .expect("the user bank is the overlay for the factory source");
        assert_eq!(
            overlay,
            paths.user_instruments_dir().join("core/forked.presets")
        );
        // User-tier sources have no overlay: their bank is already writable.
        write_folder_instrument(&paths.user_instruments_dir(), "own", "user");
        assert_eq!(
            user_preset_overlay_for_factory_source_with_paths(
                &paths,
                &paths.user_instruments_dir().join("own/dsp.lisp")
            ),
            None
        );

        let staged = root.join("staged.presets");
        write_preset_bank(&staged, "factory:core/forked", &["Factory"]);
        merge_preset_overlay_into_bank_file(&staged, &overlay).unwrap();
        assert_eq!(
            names(&read_preset_bank(&staged).unwrap().unwrap()),
            vec!["Factory", "Mine"]
        );
        // With no staged factory bank the overlay alone becomes the bank.
        let fresh = root.join("fresh.presets");
        merge_preset_overlay_into_bank_file(&fresh, &overlay).unwrap();
        assert_eq!(names(&read_preset_bank(&fresh).unwrap().unwrap()), vec!["Mine"]);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn warm_preset_name_cache_does_not_touch_the_filesystem() {
        let (paths, root) = test_paths("warm-preset-cache");
        let instrument_name = "user:cache/warm";
        write_folder_instrument(&paths.user_instruments_dir(), "cache/warm", "initial");
        save_instrument_presets_with_paths(&paths, instrument_name, &[preset("Warm")]).unwrap();

        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()
                .iter()
                .map(|preset| preset.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Warm"]
        );
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()
                .iter()
                .map(|preset| preset.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Warm"],
            "a warm lookup must not resolve, stat, or read the bank again"
        );

        let key = preset_bank_cache_key(&paths, instrument_name);
        preset_bank_cache()
            .lock()
            .unwrap()
            .invalidate_key_and_path(&key, None);
    }

    #[test]
    fn a_missing_preset_bank_is_not_cached_as_a_permanent_empty_bank() {
        let (paths, root) = test_paths("missing-preset-bank");
        let instrument_name = "user:cache/missing";
        write_folder_instrument(&paths.user_instruments_dir(), "cache/missing", "initial");

        assert!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()
                .is_empty(),
            "an instrument with no bank on disk reads as empty"
        );

        let bank_path = instrument_preset_path_with_paths(&paths, instrument_name).unwrap();
        std::fs::create_dir_all(bank_path.parent().unwrap()).unwrap();
        write_preset_bank(&bank_path, instrument_name, &["Appeared"]);
        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name).unwrap()[0].name,
            "Appeared",
            "a bank that appears after a miss must be picked up without an explicit invalidation"
        );

        let key = preset_bank_cache_key(&paths, instrument_name);
        preset_bank_cache()
            .lock()
            .unwrap()
            .invalidate_key_and_path(&key, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalidating_a_bank_path_drops_the_warm_entry_for_out_of_band_writers() {
        let (paths, root) = test_paths("invalidate-bank-path");
        let instrument_name = "user:cache/out-of-band";
        write_folder_instrument(&paths.user_instruments_dir(), "cache/out-of-band", "initial");
        save_instrument_presets_with_paths(&paths, instrument_name, &[preset("Before")]).unwrap();
        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name).unwrap()[0].name,
            "Before"
        );

        // Stand in for patch_fork::finalize, which writes a fork's bank with a
        // raw fs::write and then invalidates the path it wrote.
        let bank_path = instrument_preset_path_with_paths(&paths, instrument_name).unwrap();
        write_preset_bank(&bank_path, instrument_name, &["After"]);
        invalidate_instrument_preset_bank_cache_at(&bank_path);

        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name).unwrap()[0].name,
            "After",
            "invalidating the written path must drop the warm bank"
        );

        let key = preset_bank_cache_key(&paths, instrument_name);
        preset_bank_cache()
            .lock()
            .unwrap()
            .invalidate_key_and_path(&key, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preset_cache_is_invalidated_by_preset_and_instrument_source_saves() {
        let (paths, root) = test_paths("preset-cache-invalidation");
        let instrument_name = "user:cache/invalidation";
        write_folder_instrument(
            &paths.user_instruments_dir(),
            "cache/invalidation",
            "initial",
        );
        save_instrument_presets_with_paths(&paths, instrument_name, &[preset("First")]).unwrap();
        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()[0]
                .name,
            "First"
        );

        let bank_path = instrument_preset_path_with_paths(&paths, instrument_name).unwrap();
        write_preset_bank(&bank_path, instrument_name, &["External"]);
        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()[0]
                .name,
            "First",
            "external changes remain isolated until the instrument is reloaded"
        );

        save_instrument_with_paths(&paths, instrument_name, "updated").unwrap();
        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()[0]
                .name,
            "External",
            "saving the instrument source must invalidate its preset bank"
        );

        save_instrument_presets_with_paths(&paths, instrument_name, &[preset("Saved")]).unwrap();
        assert_eq!(
            cached_instrument_presets_with_paths(&paths, instrument_name)
                .unwrap()[0]
                .name,
            "Saved",
            "saving presets must be visible without restarting"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bare_instrument_ids_prefer_factory_and_qualified_ids_select_exact_tier() {
        let (paths, root) = test_paths("collision");
        write_folder_instrument(&paths.instruments_dir(), "shared/lead", "factory");
        write_folder_instrument(&paths.user_instruments_dir(), "shared/lead", "user");

        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "shared/lead").unwrap(),
            "factory:shared/lead"
        );
        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(
                    &paths,
                    "user:shared/lead",
                    "lisp",
                )
                .unwrap(),
            )
            .unwrap(),
            "user"
        );
        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(
                    &paths,
                    "factory:shared/lead",
                    "lisp",
                )
                .unwrap(),
            )
            .unwrap(),
            "factory"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn factory_instrument_presets_are_user_data_saved_and_loaded_from_the_user_tier() {
        let (paths, root) = test_paths("factory-presets");
        write_folder_instrument(&paths.instruments_dir(), "core/drift", "factory");

        let save_path = instrument_preset_save_path_with_paths(&paths, "factory:core/drift")
            .expect("saving presets on a factory instrument must not require forking it");
        assert_eq!(
            save_path,
            paths.user_instruments_dir().join("core/drift.presets")
        );
        std::fs::create_dir_all(save_path.parent().unwrap()).unwrap();
        std::fs::write(&save_path, "user bank").unwrap();

        // The user bank is an overlay on the factory bank, not a replacement:
        // the base path stays the factory location whether or not a factory
        // bank exists, and the user bank rides along as the overlay.
        let factory_bank = paths.instruments_dir().join("core/drift.presets");
        let bank_paths =
            instrument_preset_bank_paths_with_paths(&paths, "factory:core/drift").unwrap();
        assert_eq!(bank_paths.base, factory_bank);
        assert_eq!(bank_paths.user_overlay.as_deref(), Some(save_path.as_path()));
        std::fs::write(&factory_bank, "factory bank").unwrap();
        assert_eq!(
            instrument_preset_path_with_paths(&paths, "factory:core/drift").unwrap(),
            factory_bank
        );

        // User-tier instruments keep their bank next to the source.
        write_folder_instrument(&paths.user_instruments_dir(), "mine", "user");
        assert_eq!(
            instrument_preset_save_path_with_paths(&paths, "user:mine").unwrap(),
            paths.user_instruments_dir().join("mine.presets")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bare_instrument_id_falls_back_to_user_and_rejects_unknown_tiers() {
        let (paths, root) = test_paths("user-fallback");
        write_folder_instrument(&paths.user_instruments_dir(), "mine", "user");

        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "mine/").unwrap(),
            "user:mine"
        );
        let error = qualify_instrument_id_with_paths(&paths, "pkg:someone/mine")
            .expect_err("package ids are not part of the T3 tier resolver");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A project saved against a factory instrument keeps loading after that
    /// instrument leaves the shipped set and lives on as a user-tier copy under
    /// the same logical path; the next save re-qualifies it as `user:`.
    #[test]
    fn factory_ids_fall_through_to_a_user_tier_copy_and_requalify() {
        let (paths, root) = test_paths("factory-fallthrough");
        write_folder_instrument(&paths.user_instruments_dir(), "core/drift", "user copy");

        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(&paths, "factory:core/drift", "lisp")
                    .unwrap(),
            )
            .unwrap(),
            "user copy"
        );
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "factory:core/drift").unwrap(),
            "user:core/drift"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The fall-through is one-directional: a `user:` id with no user-tier copy
    /// never resolves into the factory tree (or the dev fixture root), so a
    /// save creates a user copy instead of overwriting shipped content, and
    /// the id never requalifies as read-only `factory:`.
    #[test]
    fn user_ids_never_fall_through_to_factory_content() {
        let (paths, root) = test_paths("user-no-fallthrough");
        write_folder_instrument(&factory_instruments_root(&paths), "core/drift", "factory");
        let fixtures = paths.dev_instrument_fixtures_dir().expect("dev layout has a fixture root");
        write_folder_instrument(&fixtures, "core/fixture-only", "fixture");
        let user_root = paths.user_instruments_dir();

        let resolved =
            resolve_instrument_storage_path_with_paths(&paths, "user:core/drift", "lisp").unwrap();
        assert!(
            resolved.starts_with(&user_root),
            "user: id resolved outside the user tier: {}",
            resolved.display()
        );
        assert!(!resolved.is_file());
        assert!(qualify_instrument_id_with_paths(&paths, "user:core/drift").is_err());

        for name in ["user:core/drift/", "user:core/fixture-only/"] {
            let writable = writable_instrument_source_path_with_paths(&paths, name).unwrap();
            assert!(
                writable.starts_with(&user_root),
                "{name} would write outside the user tier: {}",
                writable.display()
            );
            let preset = instrument_preset_save_path_with_paths(&paths, name).unwrap();
            assert!(
                preset.starts_with(&user_root),
                "{name} presets would land outside the user tier: {}",
                preset.display()
            );
        }
        save_instrument_with_paths(&paths, "user:core/drift/", "user copy").unwrap();
        assert_eq!(
            std::fs::read_to_string(factory_instruments_root(&paths).join("core/drift/dsp.lisp"))
                .unwrap(),
            "factory",
            "saving a user: id must not touch the factory source"
        );
        assert_eq!(
            std::fs::read_to_string(user_root.join("core/drift/dsp.lisp")).unwrap(),
            "user copy"
        );
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "user:core/drift").unwrap(),
            "user:core/drift"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The dev fixture tree qualifies as `factory` but resolves behind both
    /// primary tiers, so a checkout's user copy always wins over it.
    #[test]
    fn dev_fixture_root_resolves_last_and_qualifies_as_factory() {
        let (paths, root) = test_paths("fixture-root");
        let fixtures = paths.dev_instrument_fixtures_dir().expect("dev layout has a fixture root");
        write_folder_instrument(&fixtures, "core/drift", "fixture");
        write_folder_instrument(&fixtures, "core/only-here", "fixture only");

        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "factory:core/only-here").unwrap(),
            "factory:core/only-here"
        );
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "core/only-here/").unwrap(),
            "factory:core/only-here"
        );

        write_folder_instrument(&paths.user_instruments_dir(), "core/drift", "user copy");
        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(&paths, "factory:core/drift", "lisp")
                    .unwrap(),
            )
            .unwrap(),
            "user copy"
        );
        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(&paths, "core/drift/", "lisp").unwrap(),
            )
            .unwrap(),
            "user copy"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// `Synths/Syn` at release 2 (top of the folder) with the frozen
    /// `Synths/Drift` as release 1 under `versions/1`.
    fn write_versioned_syn(paths: &crate::app_paths::AppPaths, current: &str) {
        let factory = paths.instruments_dir();
        write_folder_instrument(&factory, "Synths/Syn", "release 2");
        write_folder_instrument(&factory, "Synths/Syn/versions/1", "release 1");
        std::fs::write(
            factory.join("Synths/Syn/instrument.json"),
            format!(
                r#"{{"version":1,"run_mode":"instrument",{current}"releases":{{
                    "1":{{"path":"versions/1","name":"Synths/Drift"}},
                    "2":{{"path":".","name":"Synths/Syn"}}}}}}"#
            ),
        )
        .unwrap();
    }

    fn resolved_marker(paths: &crate::app_paths::AppPaths, name: &str) -> String {
        std::fs::read_to_string(resolve_instrument_storage_path_with_paths(paths, name, "lisp").unwrap())
            .unwrap()
    }

    #[test]
    fn pinned_release_ids_resolve_to_the_release_folder() {
        let (paths, root) = test_paths("release-pinned");
        write_versioned_syn(&paths, r#""current":2,"#);
        assert_eq!(resolved_marker(&paths, "factory:Synths/Syn@1"), "release 1");
        assert_eq!(resolved_marker(&paths, "factory:Synths/Syn@2"), "release 2");
        // A pin on an old release name finds the same lineage.
        assert_eq!(resolved_marker(&paths, "factory:Synths/Drift@1"), "release 1");
        // Siblings follow the release folder.
        let factory = paths.instruments_dir();
        assert_eq!(
            resolve_instrument_storage_path_with_paths(&paths, "factory:Synths/Syn@1", "presets").unwrap(),
            factory.join("Synths/Syn/versions/1.presets")
        );
        assert_eq!(
            resolve_instrument_storage_path_with_paths(&paths, "factory:Synths/Syn@2", "presets").unwrap(),
            factory.join("Synths/Syn.presets")
        );
        assert_eq!(
            instrument_metadata_path_for_source_path(
                &resolve_instrument_storage_path_with_paths(&paths, "factory:Synths/Syn@1", "lisp").unwrap()
            )
            .unwrap(),
            factory.join("Synths/Syn/versions/1/instrument.json")
        );
        let release = instrument_release_with_paths(&paths, "factory:Synths/Syn@1").unwrap().unwrap();
        assert_eq!(
            (release.lineage.as_str(), release.release, release.name.as_str(), release.current, release.pinned),
            ("Synths/Syn", 1, "Synths/Drift", 2, Some(1))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pinning_an_unknown_release_names_the_releases_that_exist() {
        let (paths, root) = test_paths("release-unknown");
        write_versioned_syn(&paths, r#""current":2,"#);
        let error = resolve_instrument_storage_path_with_paths(&paths, "factory:Synths/Syn@7", "lisp")
            .unwrap_err()
            .to_string();
        assert!(error.contains("factory:Synths/Syn@7"), "{error}");
        assert!(error.contains("1 (Synths/Drift)") && error.contains("2 (Synths/Syn)"), "{error}");
        write_folder_instrument(&paths.instruments_dir(), "Synths/Plain", "plain");
        assert!(resolve_instrument_storage_path_with_paths(&paths, "factory:Synths/Plain@1", "lisp").is_err());
        assert!(resolve_instrument_storage_path_with_paths(&paths, "user:Synths/Syn@1", "lisp").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unpinned_ids_take_the_lowest_release_with_that_name() {
        let (paths, root) = test_paths("release-unpinned");
        write_versioned_syn(&paths, r#""current":2,"#);
        // The old folder is gone; its name still finds release 1.
        assert!(!paths.instruments_dir().join("Synths/Drift").exists());
        assert_eq!(resolved_marker(&paths, "factory:Synths/Drift"), "release 1");
        assert_eq!(resolved_marker(&paths, "Synths/Drift"), "release 1");
        assert_eq!(resolved_marker(&paths, "factory:Synths/Syn"), "release 2");
        assert_eq!(resolved_marker(&paths, "Synths/Syn/"), "release 2");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_top_folder_no_release_is_named_after_resolves_to_current() {
        let (paths, root) = test_paths("release-current");
        let factory = paths.instruments_dir();
        write_folder_instrument(&factory, "Synths/Lineage", "release 2");
        write_folder_instrument(&factory, "Synths/Lineage/versions/1", "release 1");
        std::fs::write(
            factory.join("Synths/Lineage/instrument.json"),
            r#"{"version":1,"run_mode":"instrument","current":1,"releases":{
                "1":{"path":"versions/1","name":"Synths/Old"},
                "2":{"path":".","name":"Synths/New"}}}"#,
        )
        .unwrap();
        assert_eq!(resolved_marker(&paths, "factory:Synths/Lineage"), "release 1");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unversioned_instruments_resolve_as_before() {
        let (paths, root) = test_paths("release-unversioned");
        write_versioned_syn(&paths, "");
        let factory = paths.instruments_dir();
        write_folder_instrument(&factory, "Synths/Plain", "plain");
        // An instrument.json without releases (Digi Syn today) is unversioned.
        std::fs::write(
            factory.join("Synths/Plain/instrument.json"),
            r#"{"version":1,"run_mode":"instrument","voice_controls":{"mode":"m","count":"c","legato":"l"}}"#,
        )
        .unwrap();
        assert!(instrument_release_with_paths(&paths, "factory:Synths/Plain").unwrap().is_none());
        assert_eq!(resolved_marker(&paths, "factory:Synths/Plain"), "plain");
        assert_eq!(resolved_marker(&paths, "Plain"), "plain");
        assert_eq!(
            resolve_instrument_storage_path_with_paths(&paths, "factory:Synths/Plain", "presets").unwrap(),
            factory.join("Synths/Plain.presets")
        );
        assert_eq!(qualify_instrument_id_with_paths(&paths, "Plain").unwrap(), "factory:Synths/Plain");
        // `current` defaults to the highest release.
        assert_eq!(
            instrument_release_with_paths(&paths, "factory:Synths/Syn").unwrap().unwrap().current,
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn qualify_pins_every_spelling_of_a_release_to_its_lineage() {
        let (paths, root) = test_paths("release-qualify");
        write_versioned_syn(&paths, r#""current":2,"#);
        for (input, expected) in [
            ("factory:Synths/Syn@1", "factory:Synths/Syn@1"),
            ("factory:Synths/Drift@1", "factory:Synths/Syn@1"),
            ("factory:Synths/Drift", "factory:Synths/Syn@1"),
            ("Synths/Drift", "factory:Synths/Syn@1"),
            ("Synths/Drift/", "factory:Synths/Syn@1"),
            ("factory:Synths/Syn", "factory:Synths/Syn@2"),
            ("Synths/Syn/", "factory:Synths/Syn@2"),
            ("Synths/Syn@2", "factory:Synths/Syn@2"),
        ] {
            let qualified = qualify_instrument_id_with_paths(&paths, input).unwrap();
            assert_eq!(qualified, expected, "{input}");
            // Load-time pinning never changes which release plays.
            assert_eq!(resolved_marker(&paths, &qualified), resolved_marker(&paths, input), "{input}");
            // And it is a fixed point, so load -> save writes the same id.
            assert_eq!(qualify_instrument_id_with_paths(&paths, &qualified).unwrap(), qualified);
        }
        assert!(qualify_instrument_id_with_paths(&paths, "factory:Synths/Syn@9").is_err());
        // Unversioned instruments qualify exactly as before: no pin.
        write_folder_instrument(&paths.instruments_dir(), "Synths/Plain", "plain");
        assert_eq!(qualify_instrument_id_with_paths(&paths, "Synths/Plain/").unwrap(), "factory:Synths/Plain");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Ship release 3 of `Synths/Syn` the way the spec says: move the top
    /// files into `versions/2`, put new ones at the top, bump `current`.
    fn ship_syn_release_3(paths: &crate::app_paths::AppPaths) {
        let factory = paths.instruments_dir();
        write_folder_instrument(&factory, "Synths/Syn/versions/2", "release 2");
        std::fs::write(factory.join("Synths/Syn/dsp.lisp"), "release 3").unwrap();
        std::fs::write(
            factory.join("Synths/Syn/instrument.json"),
            r#"{"version":1,"run_mode":"instrument","current":3,"releases":{
                "1":{"path":"versions/1","name":"Synths/Drift"},
                "2":{"path":"versions/2","name":"Synths/Syn"},
                "3":{"path":".","name":"Synths/Syn"}}}"#,
        )
        .unwrap();
        invalidate_instrument_release_index();
    }

    #[test]
    fn saved_pins_survive_the_next_release_and_legacy_ids_load_onto_their_release() {
        let (paths, root) = test_paths("release-round-trip");
        write_versioned_syn(&paths, r#""current":2,"#);
        // A project written before pinning names Drift and Syn unpinned.
        // Load qualifies (pins), the engine keeps that id, save qualifies it
        // again: the file now pins the release it was already playing.
        let loaded: Vec<String> = ["factory:Synths/Drift", "Synths/Syn"]
            .iter()
            .map(|id| qualify_instrument_id_with_paths(&paths, id).unwrap())
            .collect();
        assert_eq!(loaded, ["factory:Synths/Syn@1", "factory:Synths/Syn@2"]);
        let saved: Vec<String> = loaded
            .iter()
            .map(|engine_name| qualify_instrument_id_with_paths(&paths, engine_name).unwrap())
            .collect();
        assert_eq!(saved, loaded);

        ship_syn_release_3(&paths);
        // The saved pins still load the releases they were saved with...
        assert_eq!(resolved_marker(&paths, &saved[0]), "release 1");
        assert_eq!(resolved_marker(&paths, &saved[1]), "release 2");
        assert_eq!(qualify_instrument_id_with_paths(&paths, &saved[1]).unwrap(), saved[1]);
        // ...while a new track from the browser row gets the new current.
        assert_eq!(pin_instrument_for_new_track_with_paths(&paths, "Synths/Syn/"), "factory:Synths/Syn@3");
        assert_eq!(resolved_marker(&paths, "factory:Synths/Syn@3"), "release 3");
        // An unpinned legacy `Synths/Syn` predates release 3 and stays on the
        // lowest release of that name.
        assert_eq!(qualify_instrument_id_with_paths(&paths, "Synths/Syn").unwrap(), "factory:Synths/Syn@2");
        invalidate_instrument_release_index();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_tracks_pin_current_and_other_names_pass_through() {
        let (paths, root) = test_paths("release-new-track");
        write_versioned_syn(&paths, r#""current":2,"#);
        write_folder_instrument(&paths.instruments_dir(), "Synths/Plain", "plain");
        for (input, expected) in [
            ("Synths/Syn/", "factory:Synths/Syn@2"),
            ("factory:Synths/Syn", "factory:Synths/Syn@2"),
            // An explicit pin or a legacy name keeps its release.
            ("factory:Synths/Syn@1", "factory:Synths/Syn@1"),
            ("Synths/Drift", "factory:Synths/Syn@1"),
            // Unversioned or unknown names are left for the loader.
            ("Synths/Plain/", "Synths/Plain/"),
            ("user:Mine/Lead/", "user:Mine/Lead/"),
            ("nowhere", "nowhere"),
        ] {
            assert_eq!(pin_instrument_for_new_track_with_paths(&paths, input), expected, "{input}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_release_spelling_maps_to_the_lineage_id() {
        let (paths, root) = test_paths("release-lineage");
        write_versioned_syn(&paths, r#""current":2,"#);
        write_folder_instrument(&paths.instruments_dir(), "Synths/Plain", "plain");
        for input in ["factory:Synths/Drift", "Synths/Drift/", "factory:Synths/Syn@1", "Synths/Syn/", "factory:Synths/Syn@2"] {
            assert_eq!(
                instrument_lineage_id_with_paths(&paths, input).as_deref(),
                Some("factory:Synths/Syn"),
                "{input}"
            );
        }
        assert_eq!(instrument_lineage_id_with_paths(&paths, "Synths/Plain/"), None);
        assert_eq!(instrument_lineage_id_with_paths(&paths, "user:Synths/Syn"), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_ids_list_each_release_folder_with_the_names_that_load_it() {
        let (paths, root) = test_paths("release-ui-ids");
        write_versioned_syn(&paths, r#""current":2,"#);
        let factory = paths.instruments_dir();
        let ids = instrument_release_ids_with_paths(&paths);
        assert_eq!(ids.len(), 2);
        let first = ids.iter().find(|ids| ids.id == "factory:Synths/Syn@1").unwrap();
        assert_eq!(first.folder, factory.join("Synths/Syn/versions/1"));
        for alias in ["factory:Synths/Drift@1", "factory:Synths/Drift", "Synths/Drift", "Synths/Drift/"] {
            assert!(first.aliases.iter().any(|a| a == alias), "{alias} in {:?}", first.aliases);
        }
        // No alias of release 1 names the top folder, and no `versions` path
        // or bare `1` leaf ever becomes an alias.
        assert!(!first.aliases.iter().any(|a| {
            let a = a.trim_end_matches('/');
            a.ends_with("Synths/Syn") || a == "Syn"
        }));
        assert!(!first.aliases.iter().any(|a| a.contains("versions") || a == "1"));
        let second = ids.iter().find(|ids| ids.id == "factory:Synths/Syn@2").unwrap();
        assert_eq!(second.folder, factory.join("Synths/Syn"));
        for alias in ["factory:Synths/Syn", "factory:Synths/Syn/", "Synths/Syn", "Synths/Syn/", "Syn"] {
            assert!(second.aliases.iter().any(|a| a == alias), "{alias} in {:?}", second.aliases);
        }
        assert!(!second.aliases.iter().any(|a| a.contains("Drift")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_presets_use_their_release_bank_and_the_user_bank_of_their_name() {
        let (paths, root) = test_paths("release-presets");
        write_versioned_syn(&paths, r#""current":2,"#);
        let factory = paths.instruments_dir();
        let user = paths.user_instruments_dir();
        std::fs::create_dir_all(user.join("Synths")).unwrap();
        write_preset_bank(&factory.join("Synths/Syn/versions/1.presets"), "Synths/Drift/", &["Drift Factory"]);
        write_preset_bank(&factory.join("Synths/Syn.presets"), "Synths/Syn/", &["Syn Factory"]);
        // Overlays saved before the manifest existed, one per old instrument.
        write_preset_bank(&user.join("Synths/Drift.presets"), "factory:Synths/Drift", &["Drift Mine"]);
        write_preset_bank(&user.join("Synths/Syn.presets"), "factory:Synths/Syn", &["Syn Mine"]);

        for id in ["factory:Synths/Syn@1", "factory:Synths/Drift", "Synths/Drift/"] {
            let presets = cached_instrument_presets_with_paths(&paths, id).unwrap();
            assert_eq!(names(&presets), ["Drift Factory", "Drift Mine"], "{id}");
            assert_eq!(
                instrument_preset_save_path_with_paths(&paths, id).unwrap(),
                user.join("Synths/Drift.presets"),
                "{id}"
            );
        }
        for id in ["factory:Synths/Syn@2", "factory:Synths/Syn", "Synths/Syn/"] {
            let presets = cached_instrument_presets_with_paths(&paths, id).unwrap();
            assert_eq!(names(&presets), ["Syn Factory", "Syn Mine"], "{id}");
        }
        assert_eq!(
            load_user_instrument_preset_names_with_paths(&paths, "factory:Synths/Syn@1").unwrap(),
            ["Drift Mine"]
        );

        // A new user preset on the pinned release lands in that release's
        // bank only.
        let mut mine = load_user_instrument_presets_with_paths(&paths, "factory:Synths/Syn@1").unwrap();
        mine.push(preset("Drift Two"));
        save_instrument_presets_with_paths(&paths, "factory:Synths/Syn@1", &mine).unwrap();
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, "factory:Synths/Syn@1").unwrap()),
            ["Drift Factory", "Drift Mine", "Drift Two"]
        );
        assert_eq!(
            names(&cached_instrument_presets_with_paths(&paths, "factory:Synths/Syn@2").unwrap()),
            ["Syn Factory", "Syn Mine"]
        );
        assert!(!user.join("Synths/Syn@1.presets").exists());

        // A fork of the release-1 folder carries release 1's user bank.
        assert_eq!(
            user_preset_overlay_for_factory_source_with_paths(
                &paths,
                &factory.join("Synths/Syn/versions/1/dsp.lisp")
            ),
            Some(user.join("Synths/Drift.presets"))
        );

        // Retired releases are frozen: promotion only reaches `current`.
        assert!(promote_instrument_preset_to_factory_with_paths(&paths, "factory:Synths/Syn@1", &preset("P"))
            .is_err());
        assert!(promote_instrument_preset_to_factory_with_paths(&paths, "factory:Synths/Syn@2", &preset("P"))
            .is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_release_that_reuses_a_name_gets_its_own_user_bank() {
        let (paths, root) = test_paths("release-shared-name");
        write_versioned_syn(&paths, r#""current":2,"#);
        ship_syn_release_3(&paths);
        let user = paths.user_instruments_dir();
        assert_eq!(
            instrument_preset_save_path_with_paths(&paths, "factory:Synths/Syn@2").unwrap(),
            user.join("Synths/Syn.presets")
        );
        assert_eq!(
            instrument_preset_save_path_with_paths(&paths, "factory:Synths/Syn@3").unwrap(),
            user.join("Synths/Syn@3.presets")
        );
        invalidate_instrument_release_index();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_instrument_listing_skips_frozen_releases() {
        let (paths, root) = test_paths("release-listing");
        write_versioned_syn(&paths, r#""current":2,"#);
        write_folder_instrument(&paths.instruments_dir(), "Synths/Plain", "plain");
        let listed = list_saved_instruments_in(vec![paths.instruments_dir()]);
        assert_eq!(listed, ["Synths/Plain/", "Synths/Syn/"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn user_manifest(paths: &crate::app_paths::AppPaths, lineage: &str) -> serde_json::Value {
        let path = paths.user_instruments_dir().join(lineage).join("instrument.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn cutting_user_releases_freezes_each_one_and_pins_resolve_to_it() {
        let (paths, root) = test_paths("user-release-cut");
        let user = paths.user_instruments_dir();
        let top = user.join("Pads/Glass");
        write_folder_instrument(&user, "Pads/Glass", "v1");
        std::fs::write(top.join("ui.lisp"), "ui 1").unwrap();
        std::fs::write(top.join("dsp.layout.json"), "{}").unwrap();
        std::fs::write(
            top.join("instrument.json"),
            r#"{"version":1,"run_mode":"instrument","voice_controls":{"mode":"voice_mode"}}"#,
        )
        .unwrap();
        write_preset_bank(&user.join("Pads/Glass.presets"), "user:Pads/Glass", &["Bright"]);
        // Read the bank through the unversioned id so a stale cache entry
        // would show if the cut forgot to invalidate it.
        assert_eq!(names(&instrument_presets_with_paths_for_tests(&paths, "user:Pads/Glass").unwrap()), ["Bright"]);

        let cut = cut_user_release_with_paths(&paths, &top.join("dsp.lisp"), None).unwrap();
        assert_eq!(
            cut,
            UserReleaseCut {
                lineage: "Pads/Glass".into(),
                frozen: 1,
                release: 2,
                id: "user:Pads/Glass@2".into(),
                source_path: top.join("dsp.lisp"),
            }
        );
        save_instrument_with_paths(&paths, &cut.id, "v2").unwrap();
        write_preset_bank(&user.join("Pads/Glass.presets"), "user:Pads/Glass", &["Bright", "Dark"]);

        let cut = cut_user_release_with_paths(&paths, &cut.source_path, None).unwrap();
        assert_eq!((cut.frozen, cut.release, cut.id.as_str()), (2, 3, "user:Pads/Glass@3"));
        save_instrument_with_paths(&paths, &cut.id, "v3").unwrap();

        // Frozen folders hold complete releases with their own metadata.
        for (release, marker) in [(1, "v1"), (2, "v2")] {
            let frozen = top.join(format!("versions/{release}"));
            assert_eq!(std::fs::read_to_string(frozen.join("dsp.lisp")).unwrap(), marker);
            assert_eq!(std::fs::read_to_string(frozen.join("ui.lisp")).unwrap(), "ui 1");
            assert!(frozen.join("dsp.layout.json").is_file());
            let metadata: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(frozen.join("instrument.json")).unwrap()).unwrap();
            assert_eq!(metadata["voice_controls"]["mode"], "voice_mode");
            assert!(metadata.get("releases").is_none() && metadata.get("current").is_none());
        }
        let manifest = user_manifest(&paths, "Pads/Glass");
        assert_eq!(manifest["current"], 3);
        assert_eq!(
            manifest["releases"],
            serde_json::json!({
                "1": {"path": "versions/1", "name": "Pads/Glass"},
                "2": {"path": "versions/2", "name": "Pads/Glass"},
                "3": {"path": ".", "name": "Pads/Glass"},
            })
        );

        // Pins resolve to their release; the unpinned id is what was saved
        // before versioning, so release 1.
        assert_eq!(resolved_marker(&paths, "user:Pads/Glass@1"), "v1");
        assert_eq!(resolved_marker(&paths, "user:Pads/Glass@2"), "v2");
        assert_eq!(resolved_marker(&paths, "user:Pads/Glass@3"), "v3");
        assert_eq!(resolved_marker(&paths, "user:Pads/Glass"), "v1");
        // The runtime/browser spelling of the top folder is the current files.
        assert_eq!(resolved_marker(&paths, "Pads/Glass/"), "v3");
        assert!(instrument_release_with_paths(&paths, "user:Pads/Glass@9").is_err());

        // Saves pin; new tracks get current.
        assert_eq!(qualify_instrument_id_with_paths(&paths, "user:Pads/Glass").unwrap(), "user:Pads/Glass@1");
        assert_eq!(qualify_instrument_id_with_paths(&paths, "user:Pads/Glass@2").unwrap(), "user:Pads/Glass@2");
        assert_eq!(qualify_instrument_id_with_paths(&paths, "Pads/Glass/").unwrap(), "user:Pads/Glass@3");
        assert_eq!(pin_instrument_for_new_track_with_paths(&paths, "user:Pads/Glass"), "user:Pads/Glass@3");
        assert_eq!(pin_instrument_for_new_track_with_paths(&paths, "Pads/Glass/"), "user:Pads/Glass@3");
        assert_eq!(instrument_lineage_id_with_paths(&paths, "user:Pads/Glass@1").as_deref(), Some("user:Pads/Glass"));

        // Each release reads its own bank; a frozen source is never a write
        // target.
        assert_eq!(names(&instrument_presets_with_paths_for_tests(&paths, "user:Pads/Glass").unwrap()), ["Bright"]);
        assert_eq!(
            names(&instrument_presets_with_paths_for_tests(&paths, "user:Pads/Glass@2").unwrap()),
            ["Bright", "Dark"]
        );
        assert_eq!(
            instrument_preset_save_path_with_paths(&paths, "user:Pads/Glass@1").unwrap(),
            top.join("versions/1.presets")
        );
        assert_eq!(
            instrument_preset_save_path_with_paths(&paths, "user:Pads/Glass@3").unwrap(),
            user.join("Pads/Glass.presets")
        );
        let error = save_instrument_with_paths(&paths, "user:Pads/Glass@1", "clobber").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(top.join("versions/1/dsp.lisp")).unwrap(), "v1");

        // A frozen folder cannot be cut from, and listings skip `versions/`.
        assert!(cut_user_release_with_paths(&paths, &top.join("versions/1/dsp.lisp"), None).is_err());
        assert_eq!(list_saved_instruments_in(vec![user.clone()]), ["Pads/Glass/"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_user_release_spelling_maps_to_the_user_lineage() {
        let (paths, root) = test_paths("user-release-lineage");
        let user = paths.user_instruments_dir();
        write_folder_instrument(&user, "Pads/Glass", "v1");
        write_folder_instrument(&user, "Pads/Plain", "plain");
        let cut = cut_user_release_with_paths(&paths, &user.join("Pads/Glass/dsp.lisp"), None).unwrap();
        save_instrument_with_paths(&paths, &cut.id, "v2").unwrap();
        for spelling in ["user:Pads/Glass@1", "user:Pads/Glass@2", "user:Pads/Glass", "user:Pads/Glass/", "Pads/Glass/", "Pads/Glass"] {
            assert_eq!(
                instrument_lineage_id_with_paths(&paths, spelling).as_deref(),
                Some("user:Pads/Glass"),
                "{spelling}"
            );
        }
        assert_eq!(instrument_lineage_id_with_paths(&paths, "Pads/Plain/"), None);
        assert_eq!(instrument_lineage_id_with_paths(&paths, "user:Pads/Plain"), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_versioned_user_instrument_refuses_to_move_and_an_unversioned_one_still_moves() {
        let (paths, root) = test_paths("user-release-move");
        let user = paths.user_instruments_dir();
        write_folder_instrument(&user, "Pads/Glass", "v1");
        write_folder_instrument(&user, "Pads/Plain", "plain");
        std::fs::create_dir_all(user.join("Archive")).unwrap();
        cut_user_release_with_paths(&paths, &user.join("Pads/Glass/dsp.lisp"), None).unwrap();

        // (`user:Pads/Glass` means frozen release 1, refused by the
        // frozen-release guard before reaching the move guard.)
        for spelling in ["Pads/Glass/", "user:Pads/Glass@2"] {
            let error = move_saved_instrument_with_paths(&paths, spelling, "Archive").unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{spelling}");
            assert!(error.to_string().contains("user:Pads/Glass@<n>"), "{error}");
        }
        assert!(user.join("Pads/Glass/dsp.lisp").is_file());
        assert!(user.join("Pads/Glass/versions/1/dsp.lisp").is_file());
        assert!(!user.join("Archive/Glass").exists());
        assert_eq!(resolved_marker(&paths, "user:Pads/Glass@1"), "v1");

        assert_eq!(move_saved_instrument_with_paths(&paths, "Pads/Plain/", "Archive").unwrap(), "Archive/Plain/");
        assert!(user.join("Archive/Plain/dsp.lisp").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_legacy_single_file_user_instrument_moves_to_folder_form_on_its_first_cut() {
        let (paths, root) = test_paths("user-release-legacy");
        let user = paths.user_instruments_dir();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("Lead.lisp"), "v1").unwrap();
        std::fs::write(user.join("Lead.instrument.json"), r#"{"version":1,"run_mode":"effect"}"#).unwrap();
        std::fs::write(user.join("Lead.layout.json"), "{\"layout\":1}").unwrap();
        write_preset_bank(&user.join("Lead.presets"), "user:Lead", &["Saw"]);
        assert_eq!(qualify_instrument_id_with_paths(&paths, "user:Lead").unwrap(), "user:Lead");

        let cut = cut_user_release_with_paths(&paths, &user.join("Lead.lisp"), None).unwrap();
        assert_eq!((cut.frozen, cut.release, cut.id.as_str()), (1, 2, "user:Lead@2"));
        assert_eq!(cut.source_path, user.join("Lead/dsp.lisp"));
        for gone in ["Lead.lisp", "Lead.instrument.json", "Lead.layout.json"] {
            assert!(!user.join(gone).exists(), "{gone}");
        }
        assert_eq!(std::fs::read_to_string(user.join("Lead/versions/1/dsp.lisp")).unwrap(), "v1");
        assert_eq!(
            std::fs::read_to_string(user.join("Lead/versions/1/dsp.layout.json")).unwrap(),
            "{\"layout\":1}"
        );
        // The bank froze with release 1 and stays on as the current bank.
        assert!(user.join("Lead/versions/1.presets").is_file());
        assert!(user.join("Lead.presets").is_file());
        let manifest = user_manifest(&paths, "Lead");
        assert_eq!(manifest["run_mode"], "effect");
        assert_eq!(manifest["current"], 2);
        assert_eq!(manifest["releases"]["1"], serde_json::json!({"path": "versions/1", "name": "Lead"}));

        std::fs::write(&cut.source_path, "v2").unwrap();
        // Old projects' unpinned id keeps loading release 1, its bank and
        // its run mode.
        assert_eq!(resolved_marker(&paths, "user:Lead"), "v1");
        assert_eq!(resolved_marker(&paths, "user:Lead@2"), "v2");
        assert_eq!(names(&instrument_presets_with_paths_for_tests(&paths, "user:Lead").unwrap()), ["Saw"]);
        assert_eq!(
            instrument_metadata_path_for_source_path(
                &resolve_instrument_storage_path_with_paths(&paths, "user:Lead", "lisp").unwrap()
            )
            .unwrap(),
            user.join("Lead/versions/1/instrument.json")
        );
        assert_eq!(list_saved_instruments_in(vec![user.clone()]), ["Lead/"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_renaming_cut_names_the_new_release_and_keeps_the_folder() {
        let (paths, root) = test_paths("user-release-rename");
        let user = paths.user_instruments_dir();
        write_folder_instrument(&user, "Pads/Glass", "v1");
        write_folder_instrument(&user, "Pads/Taken", "other");
        let source = user.join("Pads/Glass/dsp.lisp");

        let error = cut_user_release_with_paths(&paths, &source, Some("Pads/Taken")).unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        assert!(!user.join("Pads/Glass/versions").exists());

        let cut = cut_user_release_with_paths(&paths, &source, Some("Pads/Crystal")).unwrap();
        assert_eq!(cut.id, "user:Pads/Glass@2");
        std::fs::write(&cut.source_path, "v2").unwrap();
        assert_eq!(
            user_manifest(&paths, "Pads/Glass")["releases"]["2"],
            serde_json::json!({"path": ".", "name": "Pads/Crystal"})
        );
        assert_eq!(resolved_marker(&paths, "user:Pads/Crystal"), "v2");
        assert_eq!(resolved_marker(&paths, "user:Pads/Glass"), "v1");
        assert_eq!(qualify_instrument_id_with_paths(&paths, "user:Pads/Crystal").unwrap(), "user:Pads/Glass@2");
        // A later cut without a name keeps the renamed name.
        let cut = cut_user_release_with_paths(&paths, &cut.source_path, None).unwrap();
        assert_eq!(
            user_manifest(&paths, "Pads/Glass")["releases"]["3"],
            serde_json::json!({"path": ".", "name": "Pads/Crystal"})
        );
        assert_eq!(cut.release, 3);
        // Factory ids never reach a user lineage through the index.
        assert!(instrument_release_with_paths(&paths, "factory:Pads/Glass").unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_with_releases_round_trips() {
        let source = r#"{"version":1,"run_mode":"instrument","current":2,"releases":{"1":{"path":"versions/1","name":"Synths/Drift"}}}"#;
        let metadata: InstrumentMetadataFile = serde_json::from_str(source).unwrap();
        let json: serde_json::Value = serde_json::to_value(&metadata).unwrap();
        assert_eq!(json["current"], 2);
        assert_eq!(json["releases"]["1"]["name"], "Synths/Drift");
    }

    #[test]
    fn deprecated_metadata_round_trips() {
        let source = r#"{"version":1,"run_mode":"instrument","deprecated":true,"replaced_by":"Drums/VILLAIN Kick"}"#;
        let metadata: InstrumentMetadataFile = serde_json::from_str(source).unwrap();
        let json: serde_json::Value = serde_json::to_value(&metadata).unwrap();
        assert_eq!(json["deprecated"], true);
        assert_eq!(json["replaced_by"], "Drums/VILLAIN Kick");
        // Files without the fields parse unchanged and don't gain them.
        let plain: InstrumentMetadataFile =
            serde_json::from_str(r#"{"version":1,"run_mode":"instrument"}"#).unwrap();
        let json: serde_json::Value = serde_json::to_value(&plain).unwrap();
        assert!(json.get("deprecated").is_none());
        assert!(json.get("replaced_by").is_none());
    }

    #[test]
    fn deprecated_instruments_are_unlisted_but_still_resolve() {
        let (paths, root) = test_paths("deprecated");
        let factory = paths.instruments_dir();
        write_folder_instrument(&factory, "Drums/Old Kick", "old");
        std::fs::write(
            factory.join("Drums/Old Kick/instrument.json"),
            r#"{"version":1,"run_mode":"instrument","deprecated":true,"replaced_by":"Drums/New Kick"}"#,
        )
        .unwrap();
        write_folder_instrument(&factory, "Drums/New Kick", "new");
        std::fs::write(factory.join("Drums/Old Hat.lisp"), "old hat").unwrap();
        std::fs::write(
            factory.join("Drums/Old Hat.instrument.json"),
            r#"{"version":1,"run_mode":"instrument","deprecated":true}"#,
        )
        .unwrap();
        // A versioned lineage deprecated at its top folder.
        write_versioned_syn(&paths, r#""deprecated":true,"current":2,"#);

        let listed = list_saved_instruments_in(vec![factory.clone()]);
        assert_eq!(listed, ["Drums/New Kick/"]);

        assert_eq!(resolved_marker(&paths, "Drums/Old Kick/"), "old");
        assert_eq!(resolved_marker(&paths, "factory:Drums/Old Kick"), "old");
        assert_eq!(resolved_marker(&paths, "Drums/Old Hat"), "old hat");
        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "Drums/Old Kick/").unwrap(),
            "factory:Drums/Old Kick"
        );
        assert_eq!(resolved_marker(&paths, "factory:Synths/Syn@1"), "release 1");
        assert_eq!(resolved_marker(&paths, "Synths/Drift/"), "release 1");
        assert!(qualify_instrument_id_with_paths(&paths, "Synths/Syn/").is_ok());
        assert!(instrument_source_is_deprecated(&factory.join("Drums/Old Hat.lisp")));
        assert!(!instrument_source_is_deprecated(&factory.join("Drums/New Kick/dsp.lisp")));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The shipped factory content (spec §First application): Digi Drift is
    /// Digi Syn release 1, frozen under `versions/1`.
    #[test]
    fn factory_digi_drift_is_digi_syn_release_1() {
        let paths = crate::app_paths::app_paths();
        let syn = paths.instruments_dir().join("Synths/Digi Syn");
        let source = |id: &str| resolve_instrument_storage_path_with_paths(paths, id, "lisp").unwrap();
        for id in ["factory:Synths/Digi Drift", "Synths/Digi Drift/", "factory:Synths/Digi Syn@1"] {
            assert_eq!(source(id), syn.join("versions/1/dsp.lisp"), "{id}");
            assert_eq!(qualify_instrument_id_with_paths(paths, id).unwrap(), "factory:Synths/Digi Syn@1", "{id}");
            assert_eq!(
                resolve_instrument_storage_path_with_paths(paths, id, "presets").unwrap(),
                syn.join("versions/1.presets"),
                "{id}"
            );
        }
        for id in ["factory:Synths/Digi Syn", "Synths/Digi Syn/", "factory:Synths/Digi Syn@2"] {
            assert_eq!(source(id), syn.join("dsp.lisp"), "{id}");
            assert_eq!(qualify_instrument_id_with_paths(paths, id).unwrap(), "factory:Synths/Digi Syn@2", "{id}");
        }
        // Release 1 inherits nothing from the top instrument.json: no
        // metadata of its own, so it runs exactly as Digi Drift always did.
        assert!(!syn.join("versions/1/instrument.json").exists());
        let bank: InstrumentPresetBank =
            serde_json::from_str(&std::fs::read_to_string(syn.join("versions/1.presets")).unwrap()).unwrap();
        assert!(bank.presets.iter().any(|preset| preset.name == "Woolly Res Bass"));

        let listed = list_saved_instruments_in(vec![paths.instruments_dir()]);
        assert!(listed.iter().any(|name| name == "Synths/Digi Syn/"), "{listed:?}");
        assert!(!listed.iter().any(|name| name.contains("Digi Drift") || name.contains("versions")), "{listed:?}");
    }
}

// ── Instrument compilation ──

#[cfg(test)]
mod voice_metadata_tests {
    use super::*;
    use crate::scheduled_event::{ScheduledInstrumentParam, ScheduledInstrumentParamTarget, ScheduledInstrumentParams};

    #[test]
    fn voice_metadata_validates_atomically_and_effective_values_drive_allocation() {
        let root = std::env::temp_dir().join(format!("eseq-voice-metadata-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("instrument.json");
        let mut manifest = parse_manifest(&serde_json::json!({
            "processAbi": DGEN_PROCESS_ABI_V1,
            "params": [
                {"name":"mode","cellId":7,"min":0,"max":3,"default":0},
                {"name":"count","cellId":11,"min":1,"max":32,"default":32},
                {"name":"legato","cellId":19,"min":0,"max":1,"default":1}
            ]
        }).to_string()).unwrap();
        let original = manifest.clone();
        std::fs::write(&path, r#"{"version":1,"run_mode":"instrument","voice_controls":{"mode":"missing"}}"#).unwrap();
        assert!(apply_instrument_voice_metadata(&mut manifest, Some(&root)).is_err());
        assert!(manifest.params.iter().all(|p| p.role.is_none()), "failed binding must not partly mutate roles");
        std::fs::write(&path, r#"{"version":1,"run_mode":"instrument","voice_controls":{"mode":"mode","count":"count","legato":"legato"}}"#).unwrap();
        apply_instrument_voice_metadata(&mut manifest, Some(&root)).unwrap();
        // Only the mode binds; retired count/legato names are accepted and ignored.
        assert_eq!(manifest.params.iter().filter(|p| p.role.is_some()).count(), 1);
        let descriptor = instrument_descriptor_from_manifest("test", &manifest);
        let slot = crate::effects::EffectSlotSnapshot::new_default(&descriptor, 1);
        assert!(!slot.instrument_forces_mono(&ScheduledInstrumentParams::new()));
        let mut values = ScheduledInstrumentParams::new();
        for (target, idx, value) in [
            (ScheduledInstrumentParamTarget::Synth, 7, 3.0),
            (ScheduledInstrumentParamTarget::Modulator, 7, 1.0),
        ] {
            values.push(ScheduledInstrumentParam { target, idx: idx + HEADER_SLOTS as u64, value, span: 1 });
        }
        assert!(!slot.instrument_forces_mono(&values), "unison leaves allocation to the track");
        values.push(ScheduledInstrumentParam { target: ScheduledInstrumentParamTarget::Synth, idx: 7 + HEADER_SLOTS as u64, value: 1.0, span: 1 });
        assert!(slot.instrument_forces_mono(&values));
        // Metadata is re-read for a cached artifact rather than embedded in it.
        std::fs::write(&path, r#"{"version":1,"run_mode":"instrument"}"#).unwrap();
        let mut cached = original;
        apply_instrument_voice_metadata(&mut cached, Some(&root)).unwrap();
        assert!(cached.params.iter().all(|p| p.role.is_none()));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod preset_name_index_tests {
    use super::*;

    #[test]
    fn the_index_answers_for_tree_names_and_matches_the_bank() {
        // The Instruments tree spells engines with a trailing slash.
        let indexed = indexed_instrument_preset_names("Synths/Revsynt/");
        let direct = load_instrument_preset_names("Synths/Revsynt").unwrap();
        assert_eq!(*indexed, direct);
        assert!(indexed.iter().any(|name| name == "Cut The Rev"), "{indexed:?}");
        // A second lookup is served from the index (same allocation).
        assert!(std::sync::Arc::ptr_eq(&indexed, &indexed_instrument_preset_names("Synths/Revsynt")));
    }

    #[test]
    fn an_instrument_without_presets_is_indexed_as_empty() {
        let names = indexed_instrument_preset_names("No Such Folder/No Such Instrument/");
        assert!(names.is_empty());
        assert!(std::sync::Arc::ptr_eq(&names, &indexed_instrument_preset_names("No Such Folder/No Such Instrument")));
    }
}
