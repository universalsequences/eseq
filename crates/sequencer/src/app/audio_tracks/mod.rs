//! Audio tracks, App side (docs/audio-tracks-spec.md): importing audio into
//! the content-addressed audio store, the decoded-source cache, the region
//! editing operations, and publishing each track's regions to its region
//! player.
//!
//! Regions live in the committed arrangement, so every edit here goes
//! through `edit_arrangement` and is one undo step like any clip edit.
//! [`App::sync_audio_track_playback`] runs every frame and republishes a
//! track's snapshot whenever what it would play changed — whatever caused
//! it (an edit, undo, a project load, a tempo change).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::audio_track::{AudioTrackPlayback, LockParam, PlaybackRegion, SourcePcm, DECLICK_FRAMES};
use crate::instruments::sample_core;
use crate::sequencer::{
    audio_frames_to_beats, AudioRegion, AudioSourceMeta, ClipId, InstrumentType,
    ProjectArrangement, RegionLockTarget,
};

use super::App;

/// One region as the arrangement and clip editor draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRegionView {
    pub id: u64,
    pub start_beat: f64,
    pub end_beat: f64,
    pub name: String,
    /// The waveform registry key of the region's source.
    pub waveform_key: String,
    /// Source seconds at the region's start.
    pub offset_seconds: f64,
    /// Source seconds the region spans at the current tempo.
    pub span_seconds: f64,
    pub source_seconds: f64,
    pub gain_db: f32,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub muted: bool,
    /// False while the source file is offline (drawn, silent).
    pub loaded: bool,
    /// Semitones.
    pub transpose: f32,
    /// The loop window in source seconds, when the region loops.
    pub loop_seconds: Option<(f64, f64)>,
}

/// Whether [`App::audio_region_place`] moves the region or places a copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Move,
    Copy,
}

/// The region fields [`App::audio_region_update`] sets; `None` leaves one
/// as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioRegionPatch {
    pub gain_db: Option<f32>,
    pub fade_in_seconds: Option<f32>,
    pub fade_out_seconds: Option<f32>,
    pub muted: Option<bool>,
    /// Semitones.
    pub transpose: Option<f32>,
}

/// Decoded sources and the last snapshot inputs each player was given.
#[derive(Default)]
pub struct AudioTrackRuntime {
    /// Decoded audio by store file name (`<sha256>.wav`).
    pcm: HashMap<String, Arc<SourcePcm>>,
    /// Per player slot: the regions and tempo its current snapshot was built
    /// from, so unchanged tracks are not republished.
    published: HashMap<usize, PublishedKey>,
    /// What the last sync saw; an unchanged gate skips the per-track
    /// comparison entirely (the sync runs every frame).
    sync_gate: Option<SyncGate>,
    /// Overrides the user-data audio store (tests).
    store_dir: Option<PathBuf>,
    /// Per player slot: every lock target its published snapshot carries
    /// (the regions' locks plus any earlier ones, so a removed lock still
    /// reverts its parameter to the track value).
    lock_targets: HashMap<usize, Vec<RegionLockTarget>>,
}

#[derive(Clone, PartialEq)]
struct PublishedKey {
    regions: Vec<AudioRegion>,
    lock_params: Vec<LockParam>,
    bpm: u32,
    host_rate: u32,
    loaded: usize,
}

#[derive(Clone, PartialEq)]
struct SyncGate {
    /// [`App::audio_view_key`]: song revision, tempo, decoded-source count.
    view: (u64, u32, usize),
    host_rate: u32,
    /// `(track, player slot)` per audio track.
    players: Vec<(usize, usize)>,
    /// The resolved lock parameters per slot that locks anything: a track
    /// value edit (the value a lock reverts to) or an effect rebuild
    /// republishes the snapshot.
    locks: Vec<(usize, Vec<LockParam>)>,
}

impl AudioTrackRuntime {
    /// Where sources live: an explicit override, else `ESEQ_AUDIO_STORE_DIR`
    /// (tests and tools; every App in the process, a bounce's included,
    /// agrees on it), else the user-data audio store.
    pub fn store_dir(&self) -> PathBuf {
        self.store_dir
            .clone()
            .or_else(|| std::env::var_os("ESEQ_AUDIO_STORE_DIR").map(PathBuf::from))
            .unwrap_or_else(|| crate::app_paths::app_paths().audio_dir())
    }

    pub fn set_store_dir(&mut self, dir: PathBuf) {
        self.store_dir = Some(dir);
    }

    /// The waveform registry key the timeline and clip editor draw a source
    /// with (`eseqlisp::audio::sample` registry).
    pub fn waveform_key(&self, meta: &AudioSourceMeta) -> String {
        self.store_dir().join(&meta.file).display().to_string()
    }

    fn insert_pcm(&mut self, meta: &AudioSourceMeta, stereo: Vec<f32>, sample_rate: u32) {
        let path = self.store_dir().join(&meta.file);
        eseqlisp::audio::sample::SampleBuffer::from_pcm(path, sample_rate, 2, stereo.clone()).register();
        self.pcm.insert(
            meta.file.clone(),
            Arc::new(SourcePcm { samples: stereo.into_boxed_slice(), sample_rate }),
        );
    }
}

fn sha256_file_name(bytes: &[u8]) -> String {
    format!("{:x}.wav", Sha256::digest(bytes))
}

/// A sample path's display name: the library title when it has one, else
/// the file stem.
fn source_display_name(path: &Path) -> String {
    crate::sample_db::display_title_for_sample_path(path)
        .or_else(|| path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string))
        .unwrap_or_else(|| "Audio".to_string())
}

impl App {
    pub(crate) fn seconds_per_beat(&self) -> f64 {
        60.0 / self.bpm() as f64
    }

    fn bpm(&self) -> u32 {
        self.state.transport.bpm.load(std::sync::atomic::Ordering::Relaxed).max(1)
    }

    pub fn is_audio_track(&self, track: usize) -> bool {
        self.graph.track_instrument_types.get(track) == Some(&InstrumentType::Audio)
    }

    fn require_audio_track(&self, track: usize) -> Result<(), String> {
        if self.is_audio_track(track) {
            Ok(())
        } else {
            Err(format!("Track {} is not an audio track", track + 1))
        }
    }

    /// Add an audio track at the end of the track list. Like every track it
    /// keeps (empty) scene patterns, which play nothing on an audio track;
    /// its content is its arrangement regions, and its clip lane starts
    /// empty.
    pub fn add_audio_track(&mut self) -> Result<usize, String> {
        let track = self.graph_controller().add_audio_track()?;
        self.state.reconcile_committed_arrangement_track_lanes()?;
        if let Some(mut arrangement) = self.state.committed_arrangement() {
            if arrangement.track_lanes.get(track).is_some_and(|lane| !lane.is_empty()) {
                arrangement.track_lanes[track].clear();
                self.state.set_committed_arrangement(Some(arrangement))?;
            }
        }
        Ok(track)
    }

    /// Copy `path` into the audio store (content-addressed, so importing the
    /// same file twice stores it once), decode it, and describe it. The
    /// returned meta is not yet registered with the arrangement.
    pub fn import_audio_file(&mut self, path: &Path) -> Result<AudioSourceMeta, String> {
        let path = crate::app_paths::resolve_sample_ref(path);
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
        let file = sha256_file_name(&bytes);
        let decoded = eseqlisp::audio::sample::decode_wav_bytes(&bytes)
            .map_err(|error| format!("Failed to decode {}: {error}", path.display()))?;
        let store = self.audio_tracks.store_dir();
        let stored = store.join(&file);
        if !stored.exists() {
            std::fs::create_dir_all(&store)
                .map_err(|error| format!("Failed to create {}: {error}", store.display()))?;
            // Write beside, then rename, so a crash never leaves a truncated
            // file under a content hash.
            let partial = store.join(format!("{file}.partial"));
            std::fs::write(&partial, &bytes)
                .and_then(|_| std::fs::rename(&partial, &stored))
                .map_err(|error| format!("Failed to store {}: {error}", stored.display()))?;
        }
        let meta = AudioSourceMeta {
            file,
            name: source_display_name(&path),
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            frames: decoded.frames as u64,
        };
        if !self.audio_tracks.pcm.contains_key(&meta.file) {
            let stereo = sample_core::fold_to_stereo(&decoded.samples, decoded.channels as usize);
            self.audio_tracks.insert_pcm(&meta, stereo, decoded.sample_rate);
        }
        Ok(meta)
    }

    /// Decode every source the committed arrangement references that is not
    /// cached yet (project load). A missing file is reported, and its
    /// regions stay drawn and silent.
    pub fn load_audio_sources(&mut self) -> Vec<String> {
        let sources: Vec<AudioSourceMeta> = self
            .state
            .with_committed_arrangement(|arrangement| {
                arrangement
                    .map(|arrangement| arrangement.audio_sources.values().cloned().collect())
                    .unwrap_or_default()
            });
        let mut errors = Vec::new();
        for meta in sources {
            if self.audio_tracks.pcm.contains_key(&meta.file) {
                continue;
            }
            let path = self.audio_tracks.store_dir().join(&meta.file);
            match sample_core::decode_stereo(&path) {
                Ok(decoded) => {
                    let rate = decoded.sample_rate;
                    self.audio_tracks.insert_pcm(&meta, decoded.samples, rate);
                }
                Err(error) => errors.push(format!("Audio '{}' is offline: {error}", meta.name)),
            }
        }
        errors
    }

    /// Drop samples onto audio track `track` at `beat` (spec §7): each file
    /// becomes a region spanning its whole audio, laid end to end in order.
    /// One undo step; existing regions under the drop are truncated, as a
    /// clip landing on clips is.
    pub fn audio_regions_drop(
        &mut self,
        track: usize,
        beat: f64,
        paths: &[PathBuf],
    ) -> Result<Vec<ClipId>, String> {
        self.require_audio_track(track)?;
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        if !beat.is_finite() || beat < 0.0 {
            return Err(format!("Drop beat must be finite and non-negative (got {beat})"));
        }
        let metas = paths
            .iter()
            .map(|path| self.import_audio_file(path))
            .collect::<Result<Vec<_>, _>>()?;
        let bpm = self.bpm() as f64;
        let spb = self.seconds_per_beat();
        let mut ids = Vec::new();
        self.edit_arrangement("Add audio", |arrangement, _scenes| {
            let mut start = beat;
            for meta in metas {
                let length = audio_frames_to_beats(meta.frames, meta.sample_rate, bpm);
                if !(length > 0.0) {
                    continue;
                }
                let source = arrangement.add_audio_source(meta)?;
                let id = arrangement.allocate_clip_id()?;
                let end = start + length;
                arrangement.occlude_audio_span(track, start, end, spb)?;
                arrangement.insert_audio_region_sorted(track, AudioRegion::new(id, source, start, end));
                arrangement.end_beat = arrangement.end_beat.max(end);
                ids.push(id);
                start = end;
            }
            Ok(())
        })?;
        Ok(ids)
    }

    fn locate_audio_region(
        arrangement: &ProjectArrangement,
        id: ClipId,
    ) -> Result<(usize, AudioRegion), String> {
        arrangement
            .find_audio_region(id)
            .map(|(track, region)| (track, region.clone()))
            .ok_or_else(|| format!("Audio region {} does not exist", id.0))
    }

    /// Move (or copy) a region rigidly to `start_beat`, optionally onto
    /// another audio track. It plays the same audio there and truncates what
    /// it lands on. Returns the placed region's id.
    pub fn audio_region_place(
        &mut self,
        id: ClipId,
        start_beat: f64,
        to_track: Option<usize>,
        mode: Place,
    ) -> Result<ClipId, String> {
        if let Some(track) = to_track {
            self.require_audio_track(track)?;
        }
        let spb = self.seconds_per_beat();
        let label = match mode {
            Place::Move => "Move audio",
            Place::Copy => "Copy audio",
        };
        let mut placed = id;
        self.edit_arrangement(label, |arrangement, _scenes| {
            placed = place_region_in(arrangement, id, start_beat, to_track, spb, mode)?;
            Ok(())
        })?;
        Ok(placed)
    }

    /// Paste copies of `ids` so the earliest lands at `at_beat`, keeping
    /// their spacing, onto audio track `to_track` (else each one's own
    /// track). One undo step.
    pub fn audio_regions_paste(
        &mut self,
        ids: &[ClipId],
        at_beat: f64,
        to_track: Option<usize>,
    ) -> Result<Vec<ClipId>, String> {
        if let Some(track) = to_track {
            self.require_audio_track(track)?;
        }
        if !at_beat.is_finite() || at_beat < 0.0 {
            return Err(format!("Paste beat must be finite and non-negative (got {at_beat})"));
        }
        let spb = self.seconds_per_beat();
        let ids = ids.to_vec();
        let mut pasted = Vec::new();
        self.edit_arrangement("Paste audio", |arrangement, _scenes| {
            let mut sources = ids
                .iter()
                .map(|id| Self::locate_audio_region(arrangement, *id).map(|(_, region)| region))
                .collect::<Result<Vec<_>, _>>()?;
            sources.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
            let Some(anchor) = sources.first().map(|region| region.start_beat) else {
                return Ok(());
            };
            for region in sources {
                let start = at_beat + (region.start_beat - anchor);
                pasted.push(place_region_in(arrangement, region.id, start, to_track, spb, Place::Copy)?);
            }
            Ok(())
        })?;
        Ok(pasted)
    }

    /// Copy `[start_seconds, end_seconds)` of region `id`'s SOURCE, as the
    /// clip editor's range selection does, into an arrangement clipboard of
    /// one audio region on its track. The range need not lie inside the part
    /// the region plays: any stretch of the source copies, at the region's
    /// gain and pitch, not looping. Paste lands it exactly at the cursor.
    pub fn audio_source_range_copy(
        &self,
        id: ClipId,
        start_seconds: f64,
        end_seconds: f64,
    ) -> Result<crate::app::song_region::ArrangementClipboard, String> {
        let arrangement = self.require_arrangement()?;
        let (track, region) = Self::locate_audio_region(&arrangement, id)?;
        let meta = arrangement.region_source(&region)?;
        let rate = meta.sample_rate as f64;
        let source_seconds = meta.duration_seconds();
        let start = start_seconds.max(0.0);
        let end = end_seconds.min(source_seconds);
        if !start.is_finite() || !end.is_finite() || end <= start {
            return Err("The selected audio range has no length".to_string());
        }
        let len_beats = (end - start) / (self.seconds_per_beat() * region.speed());
        let slice = AudioRegion {
            start_beat: 0.0,
            end_beat: len_beats,
            source_offset: (start * rate).round() as u64,
            loop_enabled: false,
            loop_start: 0,
            loop_end: 0,
            ..region
        };
        Ok(crate::app::song_region::ArrangementClipboard {
            len_beats,
            audio: vec![(track, vec![slice])],
            ..Default::default()
        })
    }

    /// Duplicate a region right after itself.
    pub fn audio_region_duplicate(&mut self, id: ClipId) -> Result<ClipId, String> {
        let region = self
            .require_arrangement()
            .and_then(|arrangement| Self::locate_audio_region(&arrangement, id).map(|(_, r)| r))?;
        self.audio_region_place(id, region.end_beat, None, Place::Copy)
    }

    /// Resize a region to `[start_beat, end_beat)`. The left edge trims into
    /// the source (the audio under the remaining span does not move); the
    /// right edge only changes where it stops. Growing over a neighbour
    /// truncates it.
    pub fn audio_region_resize(
        &mut self,
        id: ClipId,
        start_beat: f64,
        end_beat: f64,
    ) -> Result<(), String> {
        if !start_beat.is_finite() || start_beat < 0.0 || !(end_beat > start_beat) {
            return Err(format!(
                "An audio region needs a positive span (got [{start_beat}, {end_beat}))"
            ));
        }
        let spb = self.seconds_per_beat();
        self.edit_arrangement("Resize audio", move |arrangement, _scenes| {
            let (track, region) = Self::locate_audio_region(arrangement, id)?;
            let rate = arrangement.region_source(&region)?.sample_rate;
            let shift_frames = (start_beat - region.start_beat) * spb * rate as f64;
            let offset = region.source_offset as f64 + shift_frames;
            if offset < 0.0 {
                return Err("The region already starts at the beginning of its audio".to_string());
            }
            let mut resized = region;
            resized.start_beat = start_beat;
            resized.end_beat = end_beat;
            resized.source_offset = offset.round() as u64;
            arrangement.take_audio_region(id);
            arrangement.occlude_audio_span(track, start_beat, end_beat, spb)?;
            arrangement.insert_audio_region_sorted(track, resized);
            arrangement.end_beat = arrangement.end_beat.max(end_beat);
            Ok(())
        })
    }

    /// Split a region at `beat`; the right part keeps playing what it played.
    pub fn audio_region_split(&mut self, id: ClipId, beat: f64) -> Result<ClipId, String> {
        let spb = self.seconds_per_beat();
        let mut tail_id = None;
        self.edit_arrangement("Split audio", |arrangement, _scenes| {
            let (track, region) = Self::locate_audio_region(arrangement, id)?;
            if !(beat > region.start_beat && beat < region.end_beat) {
                return Err(format!("Beat {beat} is not inside the region"));
            }
            let rate = arrangement.region_source(&region)?.sample_rate;
            let mut tail = region.tail_at(beat, spb, rate);
            tail.id = arrangement.allocate_clip_id()?;
            tail_id = Some(tail.id);
            arrangement.take_audio_region(id);
            let mut head = region;
            head.end_beat = beat;
            arrangement.insert_audio_region_sorted(track, head);
            arrangement.insert_audio_region_sorted(track, tail);
            Ok(())
        })?;
        tail_id.ok_or_else(|| "Split produced no region".to_string())
    }

    pub fn audio_regions_delete(&mut self, ids: &[ClipId]) -> Result<(), String> {
        let ids = ids.to_vec();
        self.edit_arrangement("Delete audio", move |arrangement, _scenes| {
            for id in &ids {
                arrangement
                    .take_audio_region(*id)
                    .ok_or_else(|| format!("Audio region {} does not exist", id.0))?;
            }
            Ok(())
        })
    }

    /// Set the source window a region plays, as the clip editor's start and
    /// end markers do (Ableton clip view): the region keeps its timeline
    /// start and plays `length_seconds` of its source from `offset_seconds`,
    /// so its end moves with the length. What it grows over is truncated.
    /// With `live`, a field drag is one undo step.
    pub fn audio_region_set_window(
        &mut self,
        id: ClipId,
        offset_seconds: f64,
        length_seconds: f64,
        live: bool,
    ) -> Result<(), String> {
        if !offset_seconds.is_finite() || offset_seconds < 0.0 || !(length_seconds > 0.0) {
            return Err(format!(
                "A clip window needs a start at or after 0 and a positive length (got {offset_seconds}, {length_seconds})"
            ));
        }
        let spb = self.seconds_per_beat();
        let merge_key =
            live.then(|| crate::app::history::MergeKey::new(format!("audio-window-{}", id.0)));
        self.edit_arrangement_keyed("Audio clip window", merge_key, move |arrangement, _scenes| {
            let (track, region) = Self::locate_audio_region(arrangement, id)?;
            let meta = arrangement.region_source(&region)?;
            let offset = (offset_seconds * meta.sample_rate as f64).round() as u64;
            if offset >= meta.frames {
                return Err("The clip start is past the end of its audio".to_string());
            }
            let mut windowed = region.clone();
            windowed.source_offset = offset;
            if region.loop_enabled {
                // A looping region's window is its loop; its length on the
                // timeline is separate (extending it repeats the loop).
                let end = ((offset_seconds + length_seconds) * meta.sample_rate as f64).round() as u64;
                windowed.loop_start = offset;
                windowed.loop_end = end.min(meta.frames).max(offset + 1);
                arrangement.take_audio_region(id);
                arrangement.insert_audio_region_sorted(track, windowed);
                return Ok(());
            }
            windowed.end_beat = region.start_beat + length_seconds / (spb * region.speed());
            arrangement.take_audio_region(id);
            arrangement.occlude_audio_span(track, windowed.start_beat, windowed.end_beat, spb)?;
            arrangement.end_beat = arrangement.end_beat.max(windowed.end_beat);
            arrangement.insert_audio_region_sorted(track, windowed);
            Ok(())
        })
    }

    /// Turn a region's loop on or off. Turning it on loops the audio the
    /// region plays now, so nothing changes until the region is extended.
    pub fn audio_region_set_loop(&mut self, id: ClipId, enabled: bool) -> Result<(), String> {
        let spb = self.seconds_per_beat();
        self.edit_arrangement("Audio clip loop", move |arrangement, _scenes| {
            let (_, region) = Self::locate_audio_region(arrangement, id)?;
            let meta = arrangement.region_source(&region)?;
            let mut looped = region.clone();
            looped.loop_enabled = enabled;
            if enabled {
                let played = (region.end_beat - region.start_beat)
                    * spb
                    * meta.sample_rate as f64
                    * region.speed();
                looped.loop_start = region.source_offset;
                looped.loop_end = (region.source_offset as f64 + played).round().min(meta.frames as f64) as u64;
                if looped.loop_end <= looped.loop_start {
                    return Err("The region plays no audio to loop".to_string());
                }
            }
            *region_mut(arrangement, id)? = looped;
            Ok(())
        })
    }

    /// Apply `patch` to a region: clip gain (dB), edge fades (seconds),
    /// mute and pitch (semitones; warp off, so speed follows). With `live`,
    /// consecutive calls for the same region coalesce into one undo step (a
    /// slider or knob drag).
    pub fn audio_region_update(&mut self, id: ClipId, patch: AudioRegionPatch, live: bool) -> Result<(), String> {
        let AudioRegionPatch { gain_db, fade_in_seconds, fade_out_seconds, muted, transpose } = patch;
        for value in [gain_db, fade_in_seconds, fade_out_seconds, transpose].into_iter().flatten() {
            if !value.is_finite() {
                return Err(format!("Audio region parameter {value} must be finite"));
            }
        }
        let merge_key =
            live.then(|| crate::app::history::MergeKey::new(format!("audio-region-{}", id.0)));
        self.edit_arrangement_keyed("Audio region", merge_key, move |arrangement, _scenes| {
            let region = region_mut(arrangement, id)?;
            if let Some(gain) = gain_db {
                region.gain_db = gain.clamp(-60.0, 24.0);
            }
            if let Some(fade) = fade_in_seconds {
                region.fade_in_seconds = fade.max(0.0);
            }
            if let Some(fade) = fade_out_seconds {
                region.fade_out_seconds = fade.max(0.0);
            }
            if let Some(muted) = muted {
                region.muted = muted;
            }
            if let Some(semitones) = transpose {
                region.transpose = semitones.clamp(-48.0, 48.0);
            }
            Ok(())
        })
    }

    /// The lock target for effect parameter `slot`:`param` of the effect
    /// `track`'s slot holds now; `None` when there is no such parameter.
    fn effect_lock_target(&self, track: usize, slot: usize, param: usize) -> Option<RegionLockTarget> {
        let descriptor = self.graph.effect_descriptors.get(track)?.get(slot)?;
        (param < descriptor.params.len()).then(|| RegionLockTarget::Effect {
            slot: slot as u16,
            effect: descriptor.name.clone(),
            param: param as u16,
        })
    }

    /// Whether `track`'s slot still holds `effect` (the one rule, exact, by
    /// which a stored lock applies).
    fn slot_holds(&self, track: usize, slot: usize, effect: &str) -> bool {
        (self.graph.effect_descriptors.get(track))
            .and_then(|chain| chain.get(slot))
            .is_some_and(|descriptor| descriptor.name == effect)
    }

    /// Lock (`Some`) or unlock (`None`) each `(param, value)` of effect slot
    /// `slot` on every region in `ids`, all on `track` (eseq-a921), as one
    /// undo step: while a region plays its locks replace the track values,
    /// like step p-locks. With `live`, consecutive calls for the same
    /// regions and slot coalesce into one undo step (a knob drag).
    pub fn audio_region_set_effect_locks(
        &mut self,
        track: usize,
        ids: &[ClipId],
        slot: usize,
        values: &[(usize, Option<f32>)],
        live: bool,
    ) -> Result<(), String> {
        self.require_audio_track(track)?;
        if ids.is_empty() {
            return Err("No audio region to lock".to_string());
        }
        if values.iter().any(|(_, value)| value.is_some_and(|value| !value.is_finite())) {
            return Err("A region lock value must be finite".to_string());
        }
        let locks = values
            .iter()
            .map(|&(param, value)| {
                let target = self
                    .effect_lock_target(track, slot, param)
                    .ok_or_else(|| format!("Track {track} has no effect parameter {slot}:{param}"))?;
                Ok((target, value))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut sorted = ids.to_vec();
        sorted.sort_by_key(|id| id.0);
        let merge_key = live.then(|| {
            let ids: Vec<String> = sorted.iter().map(|id| id.0.to_string()).collect();
            crate::app::history::MergeKey::new(format!("audio-region-lock-{}-{slot}", ids.join(",")))
        });
        let label = if values.iter().any(|(_, value)| value.is_some()) {
            "Lock region parameter"
        } else {
            "Clear region lock"
        };
        self.edit_arrangement_keyed(label, merge_key, move |arrangement, _scenes| {
            for &id in &sorted {
                let (owner, region) = arrangement
                    .find_audio_region_mut(id)
                    .ok_or_else(|| format!("Audio region {} does not exist", id.0))?;
                if owner != track {
                    return Err(format!("Audio region {} is not on track {track}", id.0));
                }
                for (target, value) in &locks {
                    region.set_lock(target.clone(), *value);
                }
            }
            Ok(())
        })
    }

    /// Select `regions` on `track` for locking (`None`: no region), as the
    /// arrangement's clip selection: effect edits on that track then lock
    /// them, and its knobs show their locks.
    pub fn set_audio_lock_selection(&mut self, selection: Option<(usize, Vec<ClipId>)>) {
        let selection = selection.filter(|(track, regions)| self.is_audio_track(*track) && !regions.is_empty());
        if self.state.audio_lock_view().selection == selection {
            return;
        }
        self.state.audio_lock_view().selection = selection;
        self.refresh_audio_lock_view();
    }

    /// The regions an effect edit on `track` locks right now.
    pub fn audio_lock_regions(&self, track: usize) -> Option<Vec<ClipId>> {
        self.state.audio_lock_view().regions_for(track).map(<[ClipId]>::to_vec)
    }

    /// Recompute what the knobs show for region locks: the selection's
    /// surviving regions, the first one's locks and every locked parameter,
    /// counting only locks whose slot still holds their effect.
    pub(crate) fn refresh_audio_lock_view(&mut self) {
        let selection = self.state.audio_lock_view().selection.clone();
        let (selection, shown, locked) = self.state.with_committed_arrangement(|arrangement| {
            let (mut shown, mut locked) = (Vec::new(), Vec::new());
            let Some(arrangement) = arrangement else {
                return (None, shown, locked);
            };
            let selection = selection.and_then(|(track, regions)| {
                let lane = arrangement.audio_lane(track);
                let regions: Vec<ClipId> =
                    regions.into_iter().filter(|id| lane.iter().any(|region| region.id == *id)).collect();
                (!regions.is_empty()).then_some((track, regions))
            });
            let first = selection.as_ref().map(|(track, regions)| (*track, regions[0]));
            for (track, lane) in arrangement.audio_lanes.iter().enumerate() {
                for region in lane {
                    for lock in &region.locks {
                        let RegionLockTarget::Effect { slot, effect, param } = &lock.target;
                        let (slot, param) = (*slot as usize, *param as usize);
                        if !self.slot_holds(track, slot, effect) {
                            continue;
                        }
                        if !locked.contains(&(track, slot, param)) {
                            locked.push((track, slot, param));
                        }
                        if first == Some((track, region.id)) {
                            shown.push(((slot, param), lock.value));
                        }
                    }
                }
            }
            (selection, shown, locked)
        });
        let mut view = self.state.audio_lock_view();
        view.selection = selection;
        view.shown = shown;
        view.locked = locked;
        // The params projection checks this before taking the view's lock.
        self.state.set_audio_locks_present(view.selection.is_some() || !view.locked.is_empty());
    }

    /// Clear every lock on effect parameter `slot`:`param` across `track`'s
    /// regions (the knob menu's "Clear p-locks").
    pub fn audio_track_clear_effect_locks(&mut self, track: usize, slot: usize, param: usize) -> Result<(), String> {
        let ids: Vec<ClipId> = self
            .audio_region_effect_locks(track, slot, param)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        self.audio_region_set_effect_locks(track, &ids, slot, &[(param, None)], false)
    }

    /// `(region, value)` for every region on `track` locking effect
    /// parameter `slot`:`param` of the effect the slot holds now.
    pub fn audio_region_effect_locks(&self, track: usize, slot: usize, param: usize) -> Vec<(ClipId, f32)> {
        let Some(target) = self.effect_lock_target(track, slot, param) else {
            return Vec::new();
        };
        self.state.with_committed_arrangement(|arrangement| {
            arrangement
                .map(|arrangement| {
                    arrangement
                        .audio_lane(track)
                        .iter()
                        .filter_map(|region| Some((region.id, region.lock(&target)?)))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// An audio track has ONE fx and mixer state: every scene cell it holds
    /// shares the track sound's entities, so a row apply, a scene launch, a
    /// Stop resync or a view switch all install the same values, and an edit
    /// lands in the one sound wherever it is routed. A cell that drifted
    /// (a new scene, an older project, an undo) is re-linked here and the
    /// track sound reinstalled, so what plays is what the panel shows.
    fn link_audio_track_sounds(&mut self, players: &[(usize, usize)]) {
        // Runs every frame: read first, and take the write lock (which bumps
        // the scenes revision) only when a cell actually drifted.
        let drifted = self.state.with_project_scenes(|scenes| {
            players.iter().any(|&(track, _)| scenes.track_pool_drifted_from_track_sound(track))
        });
        if !drifted {
            return;
        }
        let relinked: Vec<usize> = self.state.with_scenes_mut(|scenes| {
            players
                .iter()
                .map(|&(track, _)| track)
                .filter(|&track| scenes.link_track_pool_to_track_sound(track))
                .collect()
        });
        for track in relinked {
            self.after_sound_repoint(track);
            self.push_track_volume(track);
            self.push_track_pan(track);
        }
        self.state.publish_scheduler_snapshot();
    }

    /// Publish each audio track's regions to its player when what it would
    /// play changed. Cheap when nothing did: one arrangement borrow and a
    /// gate comparison (song revision, tempo, sources, players).
    pub fn sync_audio_track_playback(&mut self) {
        self.audio_players.reclaim();
        let bpm = self.bpm();
        let host_rate = self.graph.sample_rate as f32;
        let players: Vec<(usize, usize)> = self
            .graph
            .track_node_ids
            .iter()
            .enumerate()
            .filter_map(|(track, nodes)| nodes.audio_player.map(|player| (track, player.slot)))
            .collect();
        // A scene launch never latches an audio track: it has no session
        // content, so its lane keeps showing (and playing) the timeline.
        self.state.set_audio_track_lane_mask(
            players.iter().filter(|(track, _)| *track < 64).fold(0, |mask, (track, _)| mask | 1 << track),
        );
        if players.is_empty() {
            return;
        }
        self.link_audio_track_sounds(&players);
        let locks = self.gate_locks(&players);
        let gate = SyncGate { view: self.audio_view_key(), host_rate: host_rate.to_bits(), players, locks };
        if self.audio_tracks.sync_gate.as_ref() == Some(&gate) {
            return;
        }
        // Forget snapshots of players that are gone (track deleted, project
        // reloaded).
        self.audio_tracks
            .published
            .retain(|slot, _| gate.players.iter().any(|&(_, live)| live == *slot));
        let players = gate.players.clone();
        let loaded = self.audio_tracks.pcm.len();
        let mut publish = Vec::new();
        self.state.with_committed_arrangement(|arrangement| {
            for &(track, slot) in &players {
                let regions = arrangement
                    .map(|arrangement| arrangement.audio_lane(track).to_vec())
                    .unwrap_or_default();
                // This snapshot's lock targets: its regions' plus the last
                // snapshot's (reverted to the track value), while they
                // still resolve.
                let (mut targets, mut lock_params): (Vec<RegionLockTarget>, Vec<LockParam>) = Default::default();
                let earlier = self.audio_tracks.lock_targets.get(&slot).into_iter().flatten();
                for target in regions.iter().flat_map(|region| region.locks.iter().map(|lock| &lock.target)).chain(earlier) {
                    if targets.contains(target) {
                        continue;
                    }
                    if let Some(param) = self.resolve_region_lock(track, target) {
                        targets.push(target.clone());
                        lock_params.push(param);
                    }
                }
                let key = PublishedKey { regions, lock_params, bpm, host_rate: host_rate.to_bits(), loaded };
                if self.audio_tracks.published.get(&slot) == Some(&key) {
                    continue;
                }
                let playback = AudioTrackPlayback {
                    regions: key
                        .regions
                        .iter()
                        .filter(|region| !region.muted)
                        .filter_map(|region| {
                            let meta = arrangement?.audio_source(region.source)?;
                            let pcm = self.audio_tracks.pcm.get(&meta.file)?;
                            let mut playing = playback_region(region, pcm, host_rate);
                            playing.locks = region
                                .locks
                                .iter()
                                .filter_map(|lock| {
                                    let index = targets.iter().position(|target| *target == lock.target)?;
                                    Some((index as u16, lock.value))
                                })
                                .collect();
                            Some(playing)
                        })
                        .collect(),
                    lock_params: key.lock_params.clone(),
                    generation: 0, // set by `publish`
                };
                publish.push((slot, key, playback, targets));
            }
        });
        for (slot, key, playback, targets) in publish {
            self.audio_players.publish(slot, playback);
            self.audio_tracks.published.insert(slot, key);
            if targets.is_empty() {
                self.audio_tracks.lock_targets.remove(&slot);
            } else {
                self.audio_tracks.lock_targets.insert(slot, targets);
            }
        }
        self.refresh_audio_lock_view();
        // The gate's locks reflect the targets just published, so the next
        // frame does not see a spurious change.
        let mut gate = gate;
        gate.locks = self.gate_locks(&gate.players);
        self.audio_tracks.sync_gate = Some(gate);
    }

    /// The gate's lock parameters: each locking slot's targets, resolved now
    /// (a track value edit or an effect rebuild moves them).
    fn gate_locks(&self, players: &[(usize, usize)]) -> Vec<(usize, Vec<LockParam>)> {
        players
            .iter()
            .filter_map(|&(track, slot)| {
                let targets = self.audio_tracks.lock_targets.get(&slot)?;
                Some((slot, targets.iter().filter_map(|target| self.resolve_region_lock(track, target)).collect()))
            })
            .collect()
    }

    /// Where a region lock on `track` writes and the track value it
    /// reverts to; `None` when its slot is gone or holds another effect.
    fn resolve_region_lock(&self, track: usize, target: &RegionLockTarget) -> Option<LockParam> {
        match target {
            RegionLockTarget::Effect { slot, effect, param } => {
                let (slot, param) = (*slot as usize, *param as usize);
                if !self.slot_holds(track, slot, effect) {
                    return None;
                }
                let state = self.state.pattern.effect_chains.get(track)?.get(slot)?;
                let id = state.param_node_id(param)?;
                Some(LockParam { logical_id: id.logical_id, idx: id.node_param_idx as u64, base: state.defaults.get(param) })
            }
        }
    }

    /// What the region views depend on beyond track topology: the committed
    /// song revision (every arrangement install bumps it), the tempo (a
    /// region's span in source seconds) and the decoded-source count.
    pub fn audio_view_key(&self) -> (u64, u32, usize) {
        (self.state.committed_song_revision(), self.bpm(), self.audio_tracks.pcm.len())
    }

    /// Every track's audio regions as the UI draws them, by track index.
    pub fn audio_region_views(&self) -> Vec<Vec<AudioRegionView>> {
        let spb = self.seconds_per_beat();
        self.state.with_committed_arrangement(|arrangement| {
            let Some(arrangement) = arrangement else {
                return Vec::new();
            };
            arrangement
                .audio_lanes
                .iter()
                .map(|lane| {
                    lane.iter()
                        .filter_map(|region| {
                            let meta = arrangement.audio_source(region.source)?;
                            let rate = meta.sample_rate.max(1) as f64;
                            Some(AudioRegionView {
                                id: region.id.0,
                                start_beat: region.start_beat,
                                end_beat: region.end_beat,
                                name: meta.name.clone(),
                                waveform_key: self.audio_tracks.waveform_key(meta),
                                offset_seconds: region.source_offset as f64 / rate,
                                span_seconds: (region.end_beat - region.start_beat) * spb * region.speed(),
                                source_seconds: meta.duration_seconds(),
                                gain_db: region.gain_db,
                                fade_in_seconds: region.fade_in_seconds,
                                fade_out_seconds: region.fade_out_seconds,
                                muted: region.muted,
                                loaded: self.audio_tracks.pcm.contains_key(&meta.file),
                                transpose: region.transpose,
                                loop_seconds: region
                                    .loop_window()
                                    .map(|(start, end)| (start / rate, end / rate)),
                            })
                        })
                        .collect()
                })
                .collect()
        })
    }

    /// Keep audio sources in `dir` instead of the user-data audio store
    /// (tests and tools that must not write into the user's data).
    pub fn set_audio_store_dir(&mut self, dir: PathBuf) {
        self.audio_tracks.set_store_dir(dir);
    }
}

/// Track `track`'s audio regions clipped to `[start_beat, end_beat)`, in
/// absolute beats. A region the window cuts into keeps playing the same
/// audio: its source offset advances to the cut. Region ids are kept (the
/// caller assigns fresh ones when it stores copies).
pub(super) fn audio_regions_in(
    arrangement: &ProjectArrangement,
    track: usize,
    start_beat: f64,
    end_beat: f64,
    seconds_per_beat: f64,
) -> Vec<AudioRegion> {
    arrangement
        .audio_lane(track)
        .iter()
        .filter_map(|region| {
            let start = region.start_beat.max(start_beat);
            let end = region.end_beat.min(end_beat);
            if end - start <= 1e-9 {
                return None;
            }
            let rate = arrangement.audio_source(region.source)?.sample_rate;
            let mut slice = region.tail_at(start, seconds_per_beat, rate);
            slice.end_beat = end;
            Some(slice)
        })
        .collect()
}

/// Slide `track`'s audio regions at or after `insert_beat` right by
/// `len_beats`, splitting one that straddles it so the moving part keeps
/// playing the audio it played there.
pub(super) fn ripple_audio_lane_right(
    arrangement: &mut ProjectArrangement,
    track: usize,
    insert_beat: f64,
    len_beats: f64,
    seconds_per_beat: f64,
) -> Result<(), String> {
    let straddling = arrangement
        .audio_lane(track)
        .iter()
        .find(|region| region.start_beat < insert_beat && region.end_beat > insert_beat)
        .cloned();
    if let Some(region) = straddling {
        let rate = arrangement.region_source(&region)?.sample_rate;
        let mut tail = region.tail_at(insert_beat, seconds_per_beat, rate);
        tail.id = arrangement.allocate_clip_id()?;
        let lane = arrangement.audio_lane_mut(track);
        if let Some(head) = lane.iter_mut().find(|candidate| candidate.id == region.id) {
            head.end_beat = insert_beat;
        }
        arrangement.insert_audio_region_sorted(track, tail);
    }
    if let Some(lane) = arrangement.audio_lanes.get_mut(track) {
        for region in lane.iter_mut().filter(|region| region.start_beat >= insert_beat) {
            region.start_beat += len_beats;
            region.end_beat += len_beats;
        }
    }
    Ok(())
}

/// A region `id`, mutably, or an error naming it.
fn region_mut(arrangement: &mut ProjectArrangement, id: ClipId) -> Result<&mut AudioRegion, String> {
    arrangement
        .find_audio_region_mut(id)
        .map(|(_, region)| region)
        .ok_or_else(|| format!("Audio region {} does not exist", id.0))
}

/// Move or copy region `id` to `start_beat` on `to_track` (else its own
/// track). Returns the placed region's id.
fn place_region_in(
    arrangement: &mut ProjectArrangement,
    id: ClipId,
    start_beat: f64,
    to_track: Option<usize>,
    seconds_per_beat: f64,
    mode: Place,
) -> Result<ClipId, String> {
    let copy = mode == Place::Copy;
    if !start_beat.is_finite() || start_beat < 0.0 {
        return Err(format!("Region start must be finite and non-negative (got {start_beat})"));
    }
    let (from_track, region) = App::locate_audio_region(arrangement, id)?;
    let track = to_track.unwrap_or(from_track);
    if !copy && track == from_track && region.start_beat == start_beat {
        return Ok(id);
    }
    let mut placed = region.clone();
    placed.start_beat = start_beat;
    placed.end_beat = start_beat + (region.end_beat - region.start_beat);
    if copy {
        placed.id = arrangement.allocate_clip_id()?;
    } else {
        arrangement.take_audio_region(id);
    }
    arrangement.occlude_audio_span(track, placed.start_beat, placed.end_beat, seconds_per_beat)?;
    let placed_id = placed.id;
    arrangement.end_beat = arrangement.end_beat.max(placed.end_beat);
    arrangement.insert_audio_region_sorted(track, placed);
    Ok(placed_id)
}

fn playback_region(region: &AudioRegion, pcm: &Arc<SourcePcm>, host_rate: f32) -> PlaybackRegion {
    let fade = |seconds: f32| (seconds as f64 * host_rate as f64).max(DECLICK_FRAMES);
    PlaybackRegion {
        start_beat: region.start_beat,
        end_beat: region.end_beat,
        source_offset: region.source_offset as f64,
        step: sample_core::playback_ratio(pcm.sample_rate, host_rate) * region.speed(),
        loop_window: region.loop_window(),
        gain: 10f32.powf(region.gain_db / 20.0),
        fade_in: fade(region.fade_in_seconds),
        fade_out: fade(region.fade_out_seconds),
        pcm: Arc::clone(pcm),
        locks: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
