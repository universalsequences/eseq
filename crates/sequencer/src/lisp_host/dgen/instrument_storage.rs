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

// ── Versioned factory instruments (docs/instrument-versioning-spec.md) ──

/// One factory instrument whose `instrument.json` carries `releases`.
struct VersionedInstrument {
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
    /// Logical path of the instrument's top folder (its lineage). A pinned id
    /// for this release is `factory:<lineage>@<release>`.
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

/// Release manifests of the factory-tier roots, scanned once per root set.
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
/// release folders). Call after writing a factory `instrument.json`
/// in-process.
pub fn invalidate_instrument_release_index() {
    instrument_release_index_cache().lock().unwrap().clear();
    resolved_walk_cache().lock().unwrap().clear();
}

/// Factory-tier instrument roots, primary first then the dev fixture root.
/// Only the factory tier ships manifests in rev 1 of the spec.
fn factory_release_roots(paths: &crate::app_paths::AppPaths) -> Vec<PathBuf> {
    let mut roots = vec![factory_instruments_root(paths)];
    roots.extend(paths.dev_instrument_fixtures_dir());
    roots
}

fn instrument_release_index(
    paths: &crate::app_paths::AppPaths,
) -> std::sync::Arc<InstrumentReleaseIndex> {
    let roots = factory_release_roots(paths);
    if let Some(index) = instrument_release_index_cache().lock().unwrap().get(&roots) {
        return index.clone();
    }
    let mut index = Vec::new();
    for root in &roots {
        scan_release_manifests(root, root, &mut index);
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
fn scan_release_manifests(root: &Path, dir: &Path, out: &mut InstrumentReleaseIndex) {
    let manifest = dir.join("instrument.json");
    if dir != root && manifest.is_file() {
        match read_release_manifest(root, dir, &manifest) {
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
        scan_release_manifests(root, &child, out);
    }
}

fn read_release_manifest(
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
    Ok(Some(VersionedInstrument { lineage, current, releases }))
}

/// Resolve an instrument id against the factory release manifests. `Ok(None)`
/// means the id names no versioned instrument and resolves as it always has.
///
/// - Pinned (`<logical>@n`): the instrument whose top folder (or, failing
///   that, some release name) is `<logical>`; unknown `n` is an error.
/// - Unpinned: the lowest release whose `name` is `<logical>` — unpinned ids
///   predate pinning, so that is the release they were saved against; if
///   only the top folder matches, `current`.
///
/// Only `factory:` ids and bare names consult the index. A pin on a `user:`
/// or `pkg:` id is an error: those tiers ship no manifests.
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
    if !matches!(tier, None | Some(InstrumentTier::Factory)) {
        return match pinned {
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument '{name}' pins a release, but only factory instruments are versioned"),
            )),
            None => Ok(None),
        };
    }
    let index = instrument_release_index(paths);
    let by_lineage = || index.iter().find(|instrument| instrument.lineage == logical);
    let by_name = || {
        index
            .iter()
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
        lineage: instrument.lineage.clone(),
        release,
        name: release_name.clone(),
        current: instrument.current,
        folder: folder.clone(),
        pinned,
    }))
}

/// The pinned id of a release: `factory:<lineage>@<release>`, what saves write.
fn pinned_release_id(release: &InstrumentRelease) -> String {
    InstrumentTier::Factory.qualify(&format!("{}@{}", release.lineage, release.release))
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
                lineage: instrument.lineage.clone(),
                release: *release,
                name: name.clone(),
                current: instrument.current,
                folder: release_folder.clone(),
                pinned: Some(*release),
            })
    })
}

/// The pinned id for an instrument folder that is a factory release folder
/// (`…/Digi Syn` or `…/Digi Syn/versions/1`); `None` for any other folder.
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
        instrument_release_with_paths(paths, &InstrumentTier::Factory.qualify(&release.name))
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
        return name.to_string();
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

/// `factory:<lineage>` for any id of a versioned instrument (any release, any
/// legacy name); `None` otherwise. Favorites and "which browser row is this
/// track" key on this so they survive new releases and renames.
pub fn instrument_lineage_id(name: &str) -> Option<String> {
    instrument_lineage_id_with_paths(crate::app_paths::app_paths(), name)
}

fn instrument_lineage_id_with_paths(paths: &crate::app_paths::AppPaths, name: &str) -> Option<String> {
    instrument_release_with_paths(paths, name)
        .ok()
        .flatten()
        .map(|release| InstrumentTier::Factory.qualify(&release.lineage))
}

/// One factory release folder with the ids that load it, for dispatching
/// its `ui.lisp` by an engine's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentReleaseIds {
    pub folder: PathBuf,
    /// The pinned id, `factory:<lineage>@<release>`.
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
            let id = InstrumentTier::Factory.qualify(&format!("{}@{release}", instrument.lineage));
            let mut candidates = Vec::new();
            for logical in &logicals {
                // A pin is only ever written on the lineage or, by hand, on
                // the name the release shipped under.
                if *logical == release_name.as_str() {
                    candidates.push(InstrumentTier::Factory.qualify(&format!("{logical}@{release}")));
                }
                candidates.push(InstrumentTier::Factory.qualify(logical));
                candidates.push(format!("{}/", InstrumentTier::Factory.qualify(logical)));
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

fn resolve_instrument_storage_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    extension: &str,
) -> io::Result<PathBuf> {
    fn is_hidden(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }

    fn collect_file_matches(dir: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                collect_file_matches(&path, file_name, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
                out.push(path);
            }
        }
    }

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
        let mut matches = Vec::new();
        if extension == "lisp" {
            collect_folder_source_matches(root, basename, &mut matches);
        }
        collect_file_matches(root, &format!("{basename}.{extension}"), &mut matches);
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

    let mut remember = |resolved: PathBuf| {
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
        return Ok(InstrumentTier::Factory.qualify(&format!("{}@{}", release.lineage, release.release)));
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

pub(in crate::lisp_host) fn collect_folder_source_matches(dir: &Path, folder_name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.starts_with('.') || !path.is_dir() {
            continue;
        }
        if name == folder_name {
            let dsp = path.join("dsp.lisp");
            if dsp.exists() {
                out.push(dsp);
            }
        }
        collect_folder_source_matches(&path, folder_name, out);
    }
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
        Some(release) => Path::new("factory")
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
        self.entries
            .retain(|_, entry| !entry.paths.iter().any(|p| p == path));
    }

    fn invalidate_key_and_path(&mut self, key: &PresetBankCacheKey, path: Option<&Path>) {
        let removed = self.entries.remove(key);
        let mut invalidated_paths = removed.map(|entry| entry.paths).unwrap_or_default();
        if let Some(path) = path {
            if !invalidated_paths.iter().any(|p| p == path) {
                invalidated_paths.push(path.to_path_buf());
            }
        }
        self.entries.retain(|_, entry| {
            !entry
                .paths
                .iter()
                .any(|p| invalidated_paths.contains(p))
        });
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

/// `Some(finished)` for an engine whose instrument declares an `@amp`
/// output; `None` means the host cannot tell and must hold the release tail.
pub fn dgen_voice_amp_finished(engine_id: usize, voice_idx: usize) -> Option<bool> {
    if engine_id >= MAX_INSTRUMENT_ENGINES || voice_idx >= MAX_VOICES {
        return None;
    }
    let slot_id = engine_id * MAX_VOICES + voice_idx;
    if DGEN_INSTRUMENT_AMP_CHANNELS[slot_id].load(Ordering::Acquire) == NO_AMP_CHANNEL {
        return None;
    }
    Some(DGEN_VOICE_AMP_STATES[slot_id].load(Ordering::Acquire) == AMP_OFF)
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

/// Wrapper process function for instrument nodes — reads from DGEN_INSTRUMENT_FNS.
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
    if *s.add(DGEN_ENABLED_PARAM_IDX) <= 0.5 {
        let nf = nframes as usize;
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
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
        return;
    }
    let engine_id = slot_id / MAX_VOICES;
    let voice_idx = slot_id % MAX_VOICES;
    if !dgen_engine_voice_runs(engine_id, voice_idx) {
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
        if !out.is_null() {
            // An idle voice is silent: zero once, then let downstream
            // routes skip it (silence propagation).
            crate::effects::silence::emit(out, output_count, nframes);
        }
        return;
    }
    let fn_ptr = DGEN_INSTRUMENT_FNS[slot_id % INSTRUMENT_REGISTRY_SIZE].load(Ordering::Acquire);
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let memory = dgen_memory_ptr(s) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        if (*out.add(0)).is_null() {
            return;
        }
        if engine_id < MAX_INSTRUMENT_ENGINES {
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
        record_dgen_voice_amp(slot_id % INSTRUMENT_REGISTRY_SIZE, out, nframes);
    } else {
        let nf = nframes as usize;
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
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
unsafe fn record_dgen_voice_amp(slot_id: usize, out: *const *mut f32, nframes: c_int) {
    let channel = DGEN_INSTRUMENT_AMP_CHANNELS[slot_id].load(Ordering::Relaxed);
    if channel == NO_AMP_CHANNEL || nframes <= 0 {
        return;
    }
    let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id].load(Ordering::Relaxed);
    if channel >= output_count {
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
    let mut buffers = vec![[0.0f32, last_amp]; output_count];
    let pointers: Vec<*mut f32> = buffers.iter_mut().map(|buffer| buffer.as_mut_ptr()).collect();
    unsafe { record_dgen_voice_amp(slot_id, pointers.as_ptr(), 2) };
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
/// and voice_cell_id = voice_index.
pub fn build_init_message_for_voice(
    slot_id: usize,
    manifest: &DGenManifest,
    voice_index: usize,
) -> Vec<f32> {
    let mut entries = init_state_entries(manifest);

    // Set voice cell to voice_index
    if let Some(cell) = manifest.voice_cell_id {
        if cell < manifest.total_memory_slots {
            entries.push((cell, voice_index as f32));
        }
    }

    // Header (10) + pairs (2 * N). Instrument nodes resolve their process
    // function through DGEN_INSTRUMENT_FNS, so the pointer chunks stay zero.
    let mut msg = Vec::with_capacity(10 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(1.0);
    msg.extend([0.0; DGEN_PROCESS_FN_CHUNKS]);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
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
    let root = crate::app_paths::app_paths().user_instruments_dir();
    let source = writable_instrument_source_path(name)?;
    if !source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("instrument '{name}' does not exist"),
        ));
    }

    let key = preset_bank_cache_key(crate::app_paths::app_paths(), name);
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
