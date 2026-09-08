//! File-backed sampler capture. Unlike compiled DSP assets, sampler files may
//! have changed since playback loaded them. A job reopens each file once.

use super::BounceCancellation;
use crate::audio::engine::HeadlessEngine;
use crate::instruments::sampler::{self, LoadedSample};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SamplerSourceSnapshot {
    paths: BTreeMap<i32, Option<PathBuf>>,
}

impl SamplerSourceSnapshot {
    /// Capture path meaning in the originating process, without opening WAVs.
    /// `None` denotes an authored blank sampler, never a failed file lookup.
    pub(crate) fn capture(
        sources: impl IntoIterator<Item = (i32, Option<PathBuf>)>,
        cancel: &BounceCancellation,
    ) -> io::Result<Self> {
        let mut paths = BTreeMap::new();
        cancel.check()?;
        for (buffer, path) in sources {
            cancel.check()?;
            let path = path.map(|path| std::path::absolute(
                crate::app_paths::resolve_sample_ref(&path),
            )).transpose()?;
            if let Some(previous) = paths.insert(buffer, path.clone()) {
                if previous != path {
                    return Err(io::Error::other(format!(
                        "Sample buffer {buffer} has conflicting source paths",
                    )));
                }
            }
        }
        Ok(Self { paths })
    }

    /// Use only in the isolated render worker. Buffer IDs in the result belong
    /// to this engine; graph teardown owns their lifetime, including on error.
    /// Analyzer inputs are returned from the same decode as the stereo audio.
    pub(crate) fn prepare(
        &self, engine: &HeadlessEngine, cancel: &BounceCancellation,
    ) -> io::Result<PreparedSamplerAssets> {
        let mut files = BTreeMap::<Option<PathBuf>, Arc<LoadedSample>>::new();
        let mut by_source_buffer = BTreeMap::new();
        cancel.check()?;
        for (source_buffer, path) in &self.paths {
            cancel.check()?;
            let canonical = path.as_ref().map(|path| path.canonicalize().map_err(|error| {
                io::Error::new(error.kind(), format!("Cannot reopen export sample {}: {error}", path.display()))
            })).transpose()?;
            let sample = if let Some(sample) = files.get(&canonical) {
                Arc::clone(sample)
            } else {
                let loaded = match &canonical {
                    Some(path) => sampler::load_wav_buffer(engine.lg_ptr.0, path)
                        .map_err(io::Error::other)?,
                    None => LoadedSample {
                        buffer_id: sampler::create_silent_buffer(engine.lg_ptr.0)
                            .map_err(io::Error::other)?,
                        name: String::new(), mono_samples: vec![0.0],
                        sample_rate: engine.sample_rate, frames: 1,
                    },
                };
                cancel.check()?;
                let sample = Arc::new(loaded);
                files.insert(canonical, Arc::clone(&sample));
                sample
            };
            by_source_buffer.insert(*source_buffer, sample);
        }
        Ok(PreparedSamplerAssets { by_source_buffer })
    }
}

pub(crate) struct PreparedSamplerAssets {
    by_source_buffer: BTreeMap<i32, Arc<LoadedSample>>,
}

impl PreparedSamplerAssets {
    /// Rebind only sampler devices in a worker-owned song copy. Rates come
    /// from the reopened files; the live project's IDs/rates are never valid
    /// substitutes, even when the same path was used by both engines.
    pub(crate) fn rebind_song(
        &self, song: &crate::sequencer::RuntimeSong,
    ) -> io::Result<crate::sequencer::RuntimeSong> {
        use crate::sequencer::InstrumentType;
        let mut song = song.clone();
        for row in &mut song.rows {
            for track_idx in 0..row.scheduler_snapshot.tracks.len() {
                let track = &row.scheduler_snapshot.tracks[track_idx];
                if track.instrument_type == InstrumentType::Sampler {
                    let binding = row.sample_ids.get_mut(track_idx).ok_or_else(|| {
                        io::Error::other(format!("Song row {} track {} has no sample binding",
                            row.id.0, track_idx + 1))
                    })?;
                    let loaded = self.sample(binding.0)?;
                    binding.0 = loaded.buffer_id;
                    binding.2 = loaded.sample_rate;
                } else if track.instrument_type == InstrumentType::Rack && track.rack_track.is_some() {
                    let snapshot = Arc::make_mut(&mut row.scheduler_snapshot);
                    let track = Arc::make_mut(&mut snapshot.tracks[track_idx]);
                    for slot in &mut track.rack_track.as_mut().unwrap().slots {
                        if slot.instrument_type == InstrumentType::Sampler {
                            if let Some(binding) = &mut slot.sample_id {
                                let loaded = self.sample(binding.0)?;
                                binding.0 = loaded.buffer_id;
                                binding.2 = loaded.sample_rate;
                            }
                        }
                    }
                }
            }
        }
        Ok(song)
    }

    pub(crate) fn sample(&self, source_buffer: i32) -> io::Result<&LoadedSample> {
        self.by_source_buffer.get(&source_buffer).map(Arc::as_ref).ok_or_else(|| {
            io::Error::other(format!("Sample buffer {source_buffer} was not captured for export"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph as graph;
    use crate::instruments::sampler::*;

    fn write_sample(path: &std::path::Path, left: f32, right: f32) {
        let spec = hound::WavSpec { channels: 2, sample_rate: 48_000,
            bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for frame in 0..1088 {
            writer.write_sample(if frame < 64 { 0.0 } else { left }).unwrap();
            writer.write_sample(if frame < 64 { 0.0 } else { right }).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn reopen_uses_current_file_once_and_retains_stereo_after_file_removal() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sample.wav");
        write_sample(&path, 0.1, 0.1);
        let cancel = BounceCancellation::default();
        let capture = SamplerSourceSnapshot::capture([
            (41, Some(path.clone())), (42, Some(root.path().join("./sample.wav"))),
        ], &cancel).unwrap();
        let bytes = serde_json::to_vec(&capture).unwrap();
        let capture: SamplerSourceSnapshot = serde_json::from_slice(&bytes).unwrap();
        write_sample(&path, 0.25, 0.75);
        let engine = crate::audio::engine::init_headless_engine(48_000, 2).unwrap();
        let prepared = capture.prepare(&engine, &cancel).unwrap();
        let sample = prepared.sample(41).unwrap();
        assert_eq!(sample.buffer_id, prepared.sample(42).unwrap().buffer_id);
        assert_eq!(sample.frames, 1024, "production leading-silence trim");
        assert_eq!(sample.sample_rate, 48_000);
        assert!(sample.mono_samples.iter().all(|value| *value == 0.5));
        assert!(prepared.sample(99).is_err());
        std::fs::remove_file(path).unwrap();
        let lg = engine.lg_ptr.0;
        let node = create_sampler_node(lg, sample.buffer_id, sample.sample_rate, "reopened").unwrap();
        unsafe {
            assert!(graph::graph_connect(lg, node.node_id, 0, 0, 0));
            assert!(graph::graph_connect(lg, node.node_id, 1, 0, 1));
            assert!(graph::prepare_graph_for_render(lg));
        }
        let mut aux = [0.0; graph::GBE_AUX_CAP];
        aux[SAMPLER_EVENT_AUX_ENABLED] = 1.0;
        aux[SAMPLER_EVENT_AUX_VELOCITY] = 1.0;
        aux[SAMPLER_EVENT_AUX_SPEED] = 1.0;
        aux[SAMPLER_EVENT_AUX_GATE_SAMPLES] = 1024.0;
        aux[SAMPLER_EVENT_AUX_END_POINT] = 1.0;
        aux[SAMPLER_EVENT_AUX_SR_HZ] = 48_000.0;
        let event = graph::GraphBlockEvent { logical_id: node.logical_id,
            frame_offset: 0, sequence: 0, kind: graph::GBE_NOTE_ON,
            aux_count: SAMPLER_EVENT_AUX_NOTE_ON_COUNT as u32, aux };
        assert!(unsafe { graph::push_block_event(lg, event) });
        let mut output = vec![0.0; 1024];
        unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
        // Allow the click-prevention envelope to settle; sampler gain defaults to 0.8.
        for frame in output[256..].chunks_exact(2) {
            assert!((frame[0] - 0.25 * 0.8).abs() < 1e-6, "{}", frame[0]);
            assert!((frame[1] - 0.75 * 0.8).abs() < 1e-6, "{}", frame[1]);
        }
        assert_eq!(unsafe { graph::graph_edit_delivery_failures(lg) }, 0);
        assert_eq!(unsafe { graph::graph_block_event_delivery_failures(lg) }, 0);
        drop(prepared);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn missing_invalid_and_cancelled_samples_fail_but_authored_blank_is_valid() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.wav");
        let cancel = BounceCancellation::default();
        let capture = SamplerSourceSnapshot::capture([(1, Some(path.clone()))], &cancel).unwrap();
        let engine = crate::audio::engine::init_headless_engine(48_000, 2).unwrap();
        let error = capture.prepare(&engine, &cancel).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("missing.wav"));
        std::fs::write(&path, "invalid WAV").unwrap();
        assert!(capture.prepare(&engine, &cancel).is_err());
        let blank = SamplerSourceSnapshot::capture([(2, None)], &cancel).unwrap()
            .prepare(&engine, &cancel).unwrap();
        assert_eq!(blank.sample(2).unwrap().mono_samples, vec![0.0]);
        cancel.cancel();
        assert_eq!(capture.prepare(&engine, &cancel).err().unwrap().kind(), io::ErrorKind::Interrupted);
        assert_eq!(SamplerSourceSnapshot::capture([], &cancel).err().unwrap().kind(), io::ErrorKind::Interrupted);
        drop(blank);
        unsafe { engine.destroy(); }
    }
}
