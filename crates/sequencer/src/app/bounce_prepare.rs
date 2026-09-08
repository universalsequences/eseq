//! Read-only preparation of the instrument code referenced by song rows.

use super::{App, EngineRegistry};
use super::fx_chain::{FxChainLeaseStore, FxChainLocator, RetainedEffectSource};
use crate::bounce::{assets::DgenSourceSnapshot, BounceCancellation};
use crate::lisp_host::DylibLease;
use crate::sequencer::{InstrumentType, RuntimeSong};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

pub(crate) struct CapturedInstrumentSource {
    pub name: String,
    pub code: DgenSourceSnapshot,
}

pub(super) struct CapturedEffectSource {
    pub locator: FxChainLocator,
    pub slot: usize,
    pub name: String,
    pub code: DgenSourceSnapshot,
}

impl App {
    pub(crate) fn capture_bounce_sampler_sources(
        &self, song: &RuntimeSong, cancel: &BounceCancellation,
    ) -> io::Result<crate::bounce::samples::SamplerSourceSnapshot> {
        capture_sampler_sources(song, cancel, |buffer, name| {
            self.capture_sampler_source_path(buffer, name).map_err(io::Error::other)
        })
    }

    pub(super) fn capture_bounce_loaded_effect_sources(
        &self, cancel: &BounceCancellation,
    ) -> io::Result<Vec<CapturedEffectSource>> {
        capture_loaded_effect_sources(&self.editor.effect_chain_leases, cancel)
    }

    pub(crate) fn capture_bounce_instrument_sources(
        &self,
        song: &RuntimeSong,
        cancel: &BounceCancellation,
    ) -> io::Result<BTreeMap<usize, CapturedInstrumentSource>> {
        capture_instrument_sources(
            &self.editor.engine_registry, &self.editor.instrument_lib_leases, song, cancel,
        )
    }
}

fn capture_sampler_sources(
    song: &RuntimeSong, cancel: &BounceCancellation,
    mut resolve: impl FnMut(i32, &str) -> io::Result<Option<std::path::PathBuf>>,
) -> io::Result<crate::bounce::samples::SamplerSourceSnapshot> {
    let mut sources = Vec::new();
    for row in &song.rows {
        cancel.check()?;
        for (track_idx, track) in row.scheduler_snapshot.tracks.iter().enumerate() {
            if track.instrument_type == InstrumentType::Sampler {
                let (buffer, name, _) = row.sample_ids.get(track_idx).ok_or_else(|| {
                    io::Error::other(format!("Song row {} track {} has no sample binding",
                        row.id.0, track_idx + 1))
                })?;
                sources.push((*buffer, resolve(*buffer, name)?));
            }
            if let Some(rack) = track.rack_track.as_ref().filter(|_| track.instrument_type == InstrumentType::Rack) {
                for slot in &rack.slots {
                    if slot.instrument_type == InstrumentType::Sampler {
                        if let Some((buffer, name, _)) = &slot.sample_id {
                            sources.push((*buffer, resolve(*buffer, name)?));
                        }
                    }
                }
            }
        }
    }
    crate::bounce::samples::SamplerSourceSnapshot::capture(sources, cancel)
}

fn capture_loaded_effect_sources(
    store: &FxChainLeaseStore, cancel: &BounceCancellation,
) -> io::Result<Vec<CapturedEffectSource>> {
    cancel.check()?;
    let mut captured = Vec::new();
    for (locator, slot, source, lease) in store.retained_sources() {
        let RetainedEffectSource::Compiled { name, source, asset_base, .. } = source else { continue; };
        let lease = lease.ok_or_else(|| io::Error::other(format!(
            "Effect {name} at {locator:?} slot {slot} has no retained compile asset inventory",
        )))?;
        let code = DgenSourceSnapshot::capture(source, asset_base.as_deref(), lease, cancel)?;
        captured.push(CapturedEffectSource { locator, slot, name: name.clone(), code });
    }
    Ok(captured)
}

fn capture_instrument_sources(
    registry: &EngineRegistry,
    leases: &[Option<DylibLease>],
    song: &RuntimeSong,
    cancel: &BounceCancellation,
) -> io::Result<BTreeMap<usize, CapturedInstrumentSource>> {
    cancel.check()?;
    let mut engines = BTreeSet::new();
    for row in &song.rows {
        cancel.check()?;
        for (track_idx, track) in row.scheduler_snapshot.tracks.iter().enumerate() {
            if track.instrument_type == InstrumentType::Custom {
                engines.insert(track.engine_id.ok_or_else(|| io::Error::other(format!(
                    "Song row {} track {} has no prepared instrument engine", row.id.0, track_idx + 1,
                )))?);
            }
            if let Some(rack) = &track.rack_track {
                for (slot_idx, slot) in rack.slots.iter().enumerate() {
                    if slot.instrument_type == InstrumentType::Custom {
                        engines.insert(slot.track_sound_state.engine_id.ok_or_else(|| io::Error::other(format!(
                            "Song row {} track {} rack slot {} has no prepared instrument engine",
                            row.id.0, track_idx + 1, slot_idx + 1,
                        )))?);
                    }
                }
            }
        }
    }
    let mut sources = BTreeMap::new();
    for engine_id in engines {
        let engine = registry.get(engine_id).ok_or_else(|| io::Error::other(format!(
            "Song references instrument engine {engine_id}, which is no longer retained",
        )))?;
        let lease = leases.get(engine.lib_index).and_then(Option::as_ref)
            .ok_or_else(|| io::Error::other(format!(
                "Instrument {} has no retained compile asset inventory", engine.name,
            )))?;
        // The registry's exact source and manifest belong to the loaded
        // engine. Never re-read a saved instrument by name: drafts and
        // unsaved replacements need not exist in the library at all.
        let code = DgenSourceSnapshot::capture(
            &engine.source, engine.manifest.asset_base.as_deref(), lease, cancel,
        )?;
        sources.insert(engine_id, CapturedInstrumentSource { name: engine.name.clone(), code });
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lisp_host::dylib_cache::{DylibCacheManager, DGenCompileKind, DGenSourceOrigin};
    use crate::sequencer::{RuntimeSongRow, SequencerState, SongRowId, default_empty_effect_chain};
    use std::sync::Arc;

    #[test]
    fn sampler_capture_resolves_each_rows_device_and_rack_sample() {
        use crate::sequencer::{RackSlotSnapshot, RackTrackSnapshot};
        let state = SequencerState::new(2, (0..2).map(|_| default_empty_effect_chain()).collect());
        let mut snapshot = (*state.latest_scheduler_snapshot()).clone();
        Arc::make_mut(&mut snapshot.tracks[0]).instrument_type = InstrumentType::Sampler;
        let track = Arc::make_mut(&mut snapshot.tracks[1]);
        track.instrument_type = InstrumentType::Rack;
        track.rack_track = Some(RackTrackSnapshot::new(vec![RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: track.instrument_run_mode,
            instrument_base_note_offset: 0.0, choke_group: None, gain: 1.0, pan: 0.0,
            mute: false, solo: false, max_polyphony: 1, param_plocks: Default::default(),
            instrument_slot: track.instrument_slot.clone(), effect_slots: Vec::new(),
            effect_descriptors: Vec::new(), custom_effect_names: Vec::new(),
            track_sound_state: Default::default(), sample_id: Some((99, "rack".into(), 48_000)),
        }], Vec::new()));
        let song = RuntimeSong {
            rows: (0..2).map(|index| RuntimeSongRow {
                id: SongRowId(index), start_beat: index as f64, scene: Some(index as usize),
                overrides: Vec::new(), resolved_pattern_ids: Vec::new(),
                resolved_sources: Vec::new(), lane_offsets: vec![0.0; 2],
                sample_ids: vec![(41 + index as i32, "track".into(), 48_000), (-1, String::new(), 48_000)],
                scheduler_snapshot: Arc::new(snapshot.clone()),
            }).collect(), end_beat: 2.0, loop_enabled: false,
        };
        let mut resolved = Vec::new();
        let capture = capture_sampler_sources(&song, &BounceCancellation::default(), |buffer, _| {
            resolved.push(buffer);
            Ok(None)
        }).unwrap();
        assert_eq!(resolved, vec![41, 99, 42, 99]);
        let engine = crate::audio::engine::init_headless_engine(44_100, 2).unwrap();
        let assets = capture.prepare(&engine, &BounceCancellation::default()).unwrap();
        let rebound = assets.rebind_song(&song).unwrap();
        for (index, row) in rebound.rows.iter().enumerate() {
            let source_buffer = 41 + index as i32;
            assert_eq!(row.sample_ids[0].0, assets.sample(source_buffer).unwrap().buffer_id);
            assert_eq!(row.sample_ids[0].2, 44_100);
            let slot = &row.scheduler_snapshot.tracks[1].rack_track.as_ref().unwrap().slots[0];
            assert_eq!(slot.sample_id.as_ref().unwrap().0, assets.sample(99).unwrap().buffer_id);
            assert_eq!(slot.sample_id.as_ref().unwrap().2, 44_100);
            assert_eq!(song.rows[index].sample_ids[0].0, source_buffer);
            assert_eq!(song.rows[index].sample_ids[0].2, 48_000);
            let original_slot = &song.rows[index].scheduler_snapshot.tracks[1]
                .rack_track.as_ref().unwrap().slots[0];
            assert_eq!(original_slot.sample_id.as_ref().unwrap().0, 99);
            assert_eq!(original_slot.sample_id.as_ref().unwrap().2, 48_000);
        }
        drop(assets);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn effect_capture_preserves_draft_source_and_host_slot_identity() {
        let original = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let manager = DylibCacheManager::new(cache.path().to_path_buf());
        std::fs::write(original.path().join("wave.json"), "[0.5]").unwrap();
        let source = "(def t (tensor @shape [1] @file \"wave.json\"))\n(out (peek t 0) 1 @name out)";
        let loaded = manager.acquire(DGenCompileKind::Effect, DGenSourceOrigin::Draft,
            source, 48_000, Some(original.path())).unwrap();
        let mut store = FxChainLeaseStore::default();
        let locator = FxChainLocator::Track(2);
        let slot = crate::effects::BUILTIN_SLOT_COUNT + 1;
        store.set(locator, slot, loaded.lease, 0).unwrap();
        store.set_source(locator, slot, Some(RetainedEffectSource::Compiled {
            name: "unsaved-effect-draft".into(), source: source.into(),
            asset_base: Some(original.path().to_path_buf()), origin: DGenSourceOrigin::Draft,
        })).unwrap();
        let frozen = capture_loaded_effect_sources(&store, &BounceCancellation::default()).unwrap();
        assert_eq!(frozen.len(), 1);
        assert_eq!(frozen[0].locator, locator);
        assert_eq!(frozen[0].slot, slot);
        assert_eq!(frozen[0].name, "unsaved-effect-draft");
        let code = frozen[0].code.freeze(&BounceCancellation::default()).unwrap();
        let refs = crate::lisp_host::dylib_cache::asset_references(&code.source).unwrap();
        std::fs::remove_file(original.path().join("wave.json")).unwrap();
        assert_eq!(std::fs::read(&refs[0]).unwrap(), b"[0.5]");
        assert_eq!(store.source(locator, slot), Some(&RetainedEffectSource::Compiled {
            name: "unsaved-effect-draft".into(), source: source.into(),
            asset_base: Some(original.path().to_path_buf()), origin: DGenSourceOrigin::Draft,
        }));
        drop(loaded.lib);
        drop(store);
    }

    #[test]
    fn song_instruments_capture_retained_draft_source_once_across_rows() {
        let original = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let manager = DylibCacheManager::new(cache.path().to_path_buf());
        std::fs::write(original.path().join("wave.json"), "[0.25]").unwrap();
        let source = "(def t (tensor @shape [1] @file \"wave.json\"))\n(out (peek t 0) 1 @name out)";
        let loaded = manager.acquire(DGenCompileKind::Instrument, DGenSourceOrigin::Draft,
            source, 48_000, Some(original.path())).unwrap();
        let mut manifest = loaded.manifest.clone();
        manifest.asset_base = Some(original.path().to_path_buf());
        let mut registry = EngineRegistry::default();
        let engine_id = registry.upsert(super::super::EngineDescriptor {
            name: "unsaved-export-draft".into(), source: source.into(), manifest,
            lib_index: 0, shared_runtime: true,
        });
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let mut snapshot = (*state.latest_scheduler_snapshot()).clone();
        let track = Arc::make_mut(&mut snapshot.tracks[0]);
        track.instrument_type = InstrumentType::Custom;
        track.engine_id = Some(engine_id);
        let snapshot = Arc::new(snapshot);
        let song = RuntimeSong {
            rows: (0..2).map(|index| RuntimeSongRow {
                id: SongRowId(index), start_beat: index as f64, scene: Some(0),
                overrides: Vec::new(), resolved_pattern_ids: Vec::new(),
                resolved_sources: Vec::new(), lane_offsets: vec![0.0],
                sample_ids: vec![(-1, String::new(), 48_000)],
                scheduler_snapshot: Arc::clone(&snapshot),
            }).collect(), end_beat: 2.0, loop_enabled: false,
        };
        let leases = vec![loaded.lease];
        let sources = capture_instrument_sources(&registry, &leases, &song,
            &BounceCancellation::default()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[&engine_id].name, "unsaved-export-draft");
        let code = sources[&engine_id].code.freeze(&BounceCancellation::default()).unwrap();
        std::fs::remove_file(original.path().join("wave.json")).unwrap();
        let refs = crate::lisp_host::dylib_cache::asset_references(&code.source).unwrap();
        assert_eq!(std::fs::read(&refs[0]).unwrap(), b"[0.25]");
        drop(loaded.lib);
        drop(leases);
    }
}
