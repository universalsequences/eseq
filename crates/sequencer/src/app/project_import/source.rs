//! Another project, opened read-only (docs/rig-spec.md §8, "Browse"): the
//! parsed file after the loader's own file-level migrations, and the tree
//! the browser lists from it. Nothing here touches the running App.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::project::{
    ProjectFile, ProjectInstanceOwner, ProjectModDestination, ProjectTrackGroup, ProjectTrackKind,
};

/// A project file opened for import: parsed, migrated like a load
/// (`prepare_project_file_for_load`), never loaded.
pub struct ImportSource {
    pub path: Option<PathBuf>,
    pub name: String,
    /// The version the file was written at (the migrations below keep
    /// `file.version` as read).
    pub file: ProjectFile,
    /// What the file-level migrations reported.
    pub notes: Vec<String>,
}

/// One scene bank of the source, as the browser lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct PeekBank {
    pub id: u64,
    /// `A`, `B`, ... by position, as the source shows it.
    pub label: String,
    pub name: Option<String>,
    pub scenes: Vec<PeekScene>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeekScene {
    pub index: usize,
    pub name: String,
}

/// A kit (drum rack group) of the source.
#[derive(Clone, Debug, PartialEq)]
pub struct PeekKit {
    pub group_id: u64,
    pub name: String,
    pub pads: usize,
    /// One entry per pad, in pad order.
    pub devices: Vec<PeekDevice>,
    /// The rack-owned sequencers (instance labels, plain script names).
    pub sequencers: Vec<String>,
    pub clips: usize,
    /// Labels of the banks where the kit plays (a scene points at one of its
    /// clips and does not park it).
    pub banks: Vec<String>,
}

/// What a pad or track plays, for the peek's device lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Sample,
    Synth,
    Modulator,
    Empty,
}

impl DeviceKind {
    /// As a track's `instrument-type` names it (the UI's icon key,
    /// `eseq.track-collapse/instrument-icon`).
    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::Sample => "sampler",
            DeviceKind::Synth => "custom",
            DeviceKind::Modulator => "modulator",
            DeviceKind::Empty => "empty",
        }
    }
}

/// One pad (or track) of the peek: its name, what plays on it (the first
/// slot of a rack) and how many rack slots it has (0 for none).
#[derive(Clone, Debug, PartialEq)]
pub struct PeekDevice {
    pub pad: String,
    pub kind: DeviceKind,
    /// "" when it would only repeat `pad`, or the sample has no name.
    pub label: String,
    pub slots: usize,
}

/// An instrument id's display name without its tier either:
/// `user:unison-warp` → `unison-warp`, `factory:drums/808 Kick@2` →
/// `808 Kick`.
fn instrument_leaf(name: &str) -> String {
    let name = crate::app::graph::instrument_display_name(name);
    match name.split_once(':') {
        Some((_, leaf)) if !leaf.is_empty() => leaf.to_string(),
        _ => name,
    }
}

use crate::sample_db::is_content_hash as looks_like_hash;

/// Sample titles for one peek: the sample library is opened read-only, once,
/// and only when a sample is stored under a content hash (titles are keyed
/// by that hash; any other stem is its own title).
#[derive(Default)]
pub struct SampleTitles {
    db: std::cell::OnceCell<Option<crate::sample_db::SampleDb>>,
}

impl SampleTitles {
    /// The library's title for a hash-named sample at `path`, else its file
    /// stem; "" for a hash the library does not know.
    fn title(&self, path: &str) -> String {
        let stem = Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or(path);
        if !looks_like_hash(stem) {
            return stem.to_string();
        }
        let db = self.db.get_or_init(|| {
            let path = crate::app_paths::app_paths().sample_db_path();
            path.is_file().then(|| crate::sample_db::SampleDb::open_read_only(&path).ok()).flatten()
        });
        db.as_ref()
            .and_then(|db| db.title_for_hash(stem).ok().flatten())
            .filter(|title| !looks_like_hash(title))
            .unwrap_or_default()
    }
}

/// A plain group of the source with its member tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct PeekGroup {
    pub group_id: u64,
    pub name: String,
    pub tracks: Vec<usize>,
    pub kits: Vec<u64>,
}

/// A track that is not a kit pad.
#[derive(Clone, Debug, PartialEq)]
pub struct PeekTrack {
    pub index: usize,
    pub name: String,
    pub device: String,
    pub group: Option<u64>,
    /// Labels of the banks where the track has a cell.
    pub banks: Vec<String>,
}

/// A project-owned sequencer (an instance of a package kind).
#[derive(Clone, Debug, PartialEq)]
pub struct PeekSequencer {
    pub id: u64,
    pub kind: String,
    pub label: String,
}

/// The read-only tree of a source project.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectPeek {
    pub name: String,
    pub version: u32,
    pub banks: Vec<PeekBank>,
    pub kits: Vec<PeekKit>,
    pub groups: Vec<PeekGroup>,
    pub tracks: Vec<PeekTrack>,
    pub sequencers: Vec<PeekSequencer>,
}

impl ImportSource {
    /// Read and migrate the project at `path`.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = crate::project::load_project_from_path(path).map_err(|error| error.to_string())?;
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Project")
            .to_string();
        let mut source = Self::from_file(&name, file)?;
        source.path = Some(path.to_path_buf());
        Ok(source)
    }

    /// Migrate an already parsed project (tests, in-memory captures).
    pub fn from_file(name: &str, mut file: ProjectFile) -> Result<Self, String> {
        let notes = super::super::projects::prepare_project_file_for_load(&mut file)?;
        Ok(Self {
            path: None,
            name: name.to_string(),
            file,
            notes,
        })
    }

    pub fn scene_count(&self) -> usize {
        self.file.patterns.len()
    }

    /// Every bank: id, name and scene range. A file without banks is one.
    pub fn banks(&self) -> Vec<(u64, Option<String>, Range<usize>)> {
        let total = self.scene_count();
        if self.file.scene_banks.is_empty() {
            return vec![(1, None, 0..total)];
        }
        let mut start = 0;
        let mut banks = Vec::with_capacity(self.file.scene_banks.len());
        for bank in &self.file.scene_banks {
            let end = (start + bank.len).min(total);
            banks.push((bank.id, bank.name.clone(), start..end));
            start = end;
        }
        // A file whose banks cover fewer scenes than it holds: the rest is
        // the last bank's, as the loader repairs it.
        if start < total {
            if let Some(last) = banks.last_mut() {
                last.2.end = total;
            }
        }
        banks
    }

    pub fn bank_range(&self, id: u64) -> Option<Range<usize>> {
        self.banks()
            .into_iter()
            .find(|(bank, _, _)| *bank == id)
            .map(|(_, _, range)| range)
    }

    fn bank_label_of_scene(&self, banks: &[(u64, Option<String>, Range<usize>)], scene: usize) -> Option<String> {
        banks
            .iter()
            .position(|(_, _, range)| range.contains(&scene))
            .map(crate::sequencer::scene_bank_auto_label)
    }

    /// Whether scene `scene` holds a cell for `track` (bare lanes are
    /// `false` in `scene_cell_presence`; legacy files hold every cell).
    pub fn cell_present(&self, scene: usize, track: usize) -> bool {
        if scene >= self.scene_count() || track >= self.file.tracks.len() {
            return false;
        }
        self.file
            .scene_cell_presence
            .get(scene)
            .and_then(|row| row.get(track))
            .copied()
            .unwrap_or(true)
    }

    pub fn group(&self, group_id: u64) -> Option<&ProjectTrackGroup> {
        self.file.groups.iter().find(|group| group.id == group_id)
    }

    /// The kit (drum rack) holding `track`, if any.
    pub fn kit_of_track(&self, track: usize) -> Option<u64> {
        self.file
            .groups
            .iter()
            .find(|group| group.is_rack() && group.members.contains(&track))
            .map(|group| group.id)
    }

    /// The plain group holding `track`, if any.
    pub fn plain_group_of_track(&self, track: usize) -> Option<u64> {
        self.file
            .groups
            .iter()
            .find(|group| !group.is_rack() && group.members.contains(&track))
            .map(|group| group.id)
    }

    /// The clip scene `scene` points kit `group_id` at.
    pub fn scene_rack_clip(&self, scene: usize, group_id: u64) -> Option<u64> {
        self.file
            .scene_rack_clips
            .get(scene)?
            .iter()
            .find(|(group, _)| *group == group_id)
            .map(|(_, clip)| *clip)
    }

    pub fn kit_parked(&self, scene: usize, group_id: u64) -> bool {
        self.file
            .scene_parked_kits
            .get(scene)
            .is_some_and(|parked| parked.contains(&group_id))
    }

    /// Whether kit `group_id` sounds in scene `scene`: a clip rack when the
    /// scene points at one of its clips and does not park it; a legacy rack
    /// when any member has a cell there.
    pub fn kit_plays_in_scene(&self, scene: usize, group_id: u64) -> bool {
        let Some(group) = self.group(group_id) else {
            return false;
        };
        if self.kit_parked(scene, group_id) {
            return false;
        }
        let legacy = group.rack.as_ref().is_some_and(|rack| rack.clips.is_empty());
        if legacy {
            group.members.iter().any(|track| self.cell_present(scene, *track))
        } else {
            self.scene_rack_clip(scene, group_id).is_some()
        }
    }

    /// The scene whose state stands for "the project as saved": the saved
    /// current scene.
    pub fn current_scene(&self) -> usize {
        self.file.current_pattern.min(self.scene_count().saturating_sub(1))
    }

    pub fn track_name(&self, track: usize) -> String {
        self.file
            .tracks
            .get(track)
            .and_then(|track| track.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("Track {}", track + 1))
    }

    /// A one-line device label for `track`: its instrument (a rack's first
    /// slot, with a count of the others), "" for an unnamed sample.
    pub fn device_label(&self, track: usize, titles: &SampleTitles) -> String {
        let device = self.device(track, titles);
        match device.slots {
            0 | 1 => device.label,
            slots if device.label.is_empty() => format!("{slots} slots"),
            slots => format!("{} +{}", device.label, slots - 1),
        }
    }

    /// What plays on `track`: its kind, a readable name (an instrument's
    /// leaf name without its namespace or version, a sample's title; "" for
    /// a sample stored under a content hash) and how many rack slots.
    pub fn device(&self, track: usize, titles: &SampleTitles) -> PeekDevice {
        let pad = self.track_name(track);
        let Some(project_track) = self.file.tracks.get(track) else {
            return PeekDevice { pad, kind: DeviceKind::Empty, label: String::new(), slots: 0 };
        };
        let (kind, label, slots) = match &project_track.kind {
            ProjectTrackKind::Empty => (DeviceKind::Empty, String::new(), 0),
            ProjectTrackKind::Modulator => (DeviceKind::Modulator, String::new(), 0),
            ProjectTrackKind::Custom { instrument_name } => {
                (DeviceKind::Synth, instrument_leaf(instrument_name), 1)
            }
            ProjectTrackKind::Sampler { sample_path } => {
                (DeviceKind::Sample, sample_path.as_deref().map(|path| titles.title(path)).unwrap_or_default(), 1)
            }
            ProjectTrackKind::Rack { slots, .. } => {
                let first = slots.first().map(|slot| match &slot.instrument_name {
                    Some(name) => (DeviceKind::Synth, instrument_leaf(name)),
                    None => {
                        let title = slot
                            .sample_name
                            .clone()
                            .filter(|name| !looks_like_hash(name))
                            .or_else(|| slot.sample_path.as_deref().map(|path| titles.title(path)))
                            .unwrap_or_default();
                        (DeviceKind::Sample, title)
                    }
                });
                let (kind, label) = first.unwrap_or((DeviceKind::Synth, String::new()));
                (kind, label, slots.len())
            }
        };
        // A label the pad's own name already says is noise.
        let label = if pad.to_lowercase().contains(&label.to_lowercase()) { String::new() } else { label };
        PeekDevice { pad, kind, label, slots }
    }

    /// Mod cables of scene `scene`, as `(source track, destination)`.
    pub(super) fn scene_mod_connections(
        &self,
        scene: usize,
    ) -> Vec<(crate::project::ProjectModConnection, Option<ProjectModDestination>)> {
        self.file
            .patterns
            .get(scene)
            .map(|pattern| {
                pattern
                    .mod_connections
                    .iter()
                    .map(|connection| {
                        let destination = connection
                            .destination
                            .or(connection.dest_track.map(ProjectModDestination::Track));
                        (*connection, destination)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The browser's tree of this project.
    pub fn peek(&self) -> ProjectPeek {
        let titles = SampleTitles::default();
        let banks = self.banks();
        let label_set = |scenes: &mut dyn Iterator<Item = usize>| {
            let mut labels: Vec<String> = Vec::new();
            for scene in scenes {
                if let Some(label) = self.bank_label_of_scene(&banks, scene) {
                    if !labels.contains(&label) {
                        labels.push(label);
                    }
                }
            }
            labels
        };
        let peek_banks = banks
            .iter()
            .enumerate()
            .map(|(position, (id, name, range))| PeekBank {
                id: *id,
                label: crate::sequencer::scene_bank_auto_label(position),
                name: name.clone(),
                scenes: range
                    .clone()
                    .map(|index| PeekScene { index, name: format!("Scene {}", index + 1) })
                    .collect(),
            })
            .collect();
        let kits = self
            .file
            .groups
            .iter()
            .filter_map(|group| {
                let rack = group.rack.as_ref()?;
                let devices = rack
                    .pads
                    .iter()
                    .filter_map(|pad| Some(self.device(*group.members.get(pad.member)?, &titles)))
                    .collect();
                let mut sequencers: Vec<String> = self
                    .file
                    .instances
                    .list
                    .iter()
                    .filter(|instance| instance.owner == ProjectInstanceOwner::Rack(group.id))
                    .map(|instance| instance.label.clone())
                    .collect();
                sequencers.extend(rack.sequencers.iter().map(|seq| seq.sequencer_name.clone()));
                Some(PeekKit {
                    group_id: group.id,
                    name: group.name.clone(),
                    pads: rack.pads.len(),
                    devices,
                    sequencers,
                    clips: rack.clips.len(),
                    banks: label_set(
                        &mut (0..self.scene_count())
                            .filter(|scene| self.kit_plays_in_scene(*scene, group.id)),
                    ),
                })
            })
            .collect();
        let groups = self
            .file
            .groups
            .iter()
            .filter(|group| !group.is_rack())
            .map(|group| PeekGroup {
                group_id: group.id,
                name: group.name.clone(),
                tracks: group.members.clone(),
                kits: group.rack_members.clone(),
            })
            .collect();
        let tracks = (0..self.file.tracks.len())
            .filter(|track| self.kit_of_track(*track).is_none())
            .map(|track| PeekTrack {
                index: track,
                name: self.track_name(track),
                device: self.device_label(track, &titles),
                group: self.plain_group_of_track(track),
                banks: label_set(
                    &mut (0..self.scene_count()).filter(|scene| self.cell_present(*scene, track)),
                ),
            })
            .collect();
        let sequencers = self
            .file
            .instances
            .list
            .iter()
            .filter(|instance| instance.owner == ProjectInstanceOwner::Project)
            .map(|instance| PeekSequencer {
                id: instance.id,
                kind: instance.kind.clone(),
                label: instance.label.clone(),
            })
            .collect();
        ProjectPeek {
            name: self.name.clone(),
            version: self.file.version,
            banks: peek_banks,
            kits,
            groups,
            tracks,
            sequencers,
        }
    }
}
