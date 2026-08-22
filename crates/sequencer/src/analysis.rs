use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

#[derive(Clone, Debug)]
pub struct AnalysisJob {
    pub buffer_id: i32,
    pub samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
}

#[derive(Clone, Debug)]
pub struct AnalysisResult {
    pub buffer_id: i32,
    pub bpm: f32,
    pub bpm_confidence: f32,
    pub onsets_frames: Vec<u32>,
    pub downbeat_frame: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum AnalysisEntry {
    Pending,
    Ready(AnalysisResult),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct SliceTableShared {
    pub onsets_frames: Vec<u32>,
    pub sample_len_frames: u32,
    pub sample_rate: u32,
}

impl SliceTableShared {
    pub fn from_result(result: &AnalysisResult, sample_len_frames: u32, sample_rate: u32) -> Self {
        Self {
            onsets_frames: result.onsets_frames.clone(),
            sample_len_frames,
            sample_rate,
        }
    }

    /// Iterate transient slice starts without allocating on the audio thread.
    /// Aubio has already applied its strength threshold; sensitivity controls
    /// the second half of transient filtering, minimum spacing. At maximum
    /// sensitivity the spacing equals aubio's 40 ms minimum, while zero
    /// sensitivity keeps transients at least 500 ms apart.
    pub fn slice_starts(&self, sensitivity: f32) -> impl Iterator<Item = u32> + '_ {
        let sensitivity = if sensitivity.is_finite() {
            sensitivity.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let min_spacing_ms = 500.0 + (40.0 - 500.0) * sensitivity;
        let min_spacing = (self.sample_rate as f32 * min_spacing_ms / 1000.0).round() as u32;
        let mut last = Some(0_u32);
        std::iter::once(0).chain(
            self.onsets_frames
                .iter()
                .copied()
                .filter(move |frame| {
                    if *frame == 0 || *frame >= self.sample_len_frames {
                        return false;
                    }
                    let keep = last.map_or(true, |previous| frame.saturating_sub(previous) >= min_spacing);
                    if keep {
                        last = Some(*frame);
                    }
                    keep
                }),
        )
    }
}

/// Warp and slice resolution intentionally share the same immutable analysis table.
pub type OnsetTableShared = SliceTableShared;

#[derive(Clone, Default)]
pub struct AnalysisCache {
    inner: Arc<RwLock<HashMap<i32, Arc<AnalysisEntry>>>>,
    tables: Arc<RwLock<HashMap<i32, Arc<OnsetTableShared>>>>,
    generation: Arc<AtomicU64>,
}

impl AnalysisCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_pending(&self, buffer_id: i32) {
        self.inner
            .write()
            .unwrap()
            .insert(buffer_id, Arc::new(AnalysisEntry::Pending));
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn insert_ready(&self, result: AnalysisResult, sample_len_frames: u32, sample_rate: u32) {
        let buffer_id = result.buffer_id;
        let table = Arc::new(SliceTableShared::from_result(
            &result,
            sample_len_frames,
            sample_rate,
        ));
        self.tables.write().unwrap().insert(buffer_id, table);
        self.inner
            .write()
            .unwrap()
            .insert(buffer_id, Arc::new(AnalysisEntry::Ready(result)));
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn insert_failed(&self, buffer_id: i32, error: impl Into<String>) {
        self.inner
            .write()
            .unwrap()
            .insert(buffer_id, Arc::new(AnalysisEntry::Failed(error.into())));
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn get(&self, buffer_id: i32) -> Option<Arc<AnalysisEntry>> {
        self.inner.read().unwrap().get(&buffer_id).cloned()
    }

    pub fn table(&self, buffer_id: i32) -> Option<Arc<OnsetTableShared>> {
        self.tables.read().unwrap().get(&buffer_id).cloned()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct AnalysisService {
    cache: AnalysisCache,
    tx: Sender<AnalysisJob>,
}

impl AnalysisService {
    pub fn new() -> Self {
        let cache = AnalysisCache::new();
        let (tx, rx) = mpsc::channel::<AnalysisJob>();
        let worker_cache = cache.clone();
        let _ = thread::Builder::new()
            .name("sample-analysis-worker".to_string())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let sample_len_frames = job.samples.len() as u32;
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        analyze(&job.samples, job.sample_rate, job.buffer_id)
                    }));
                    match result {
                        Ok(Ok(result)) => worker_cache.insert_ready(
                            result,
                            sample_len_frames,
                            job.sample_rate,
                        ),
                        Ok(Err(error)) => worker_cache.insert_failed(job.buffer_id, error),
                        Err(_) => worker_cache.insert_failed(job.buffer_id, "analysis panicked"),
                    }
                }
            });
        Self { cache, tx }
    }

    pub fn cache(&self) -> &AnalysisCache {
        &self.cache
    }

    pub fn submit(&self, job: AnalysisJob) {
        self.cache.insert_pending(job.buffer_id);
        if let Err(error) = self.tx.send(job) {
            self.cache
                .insert_failed(error.0.buffer_id, "analysis worker is not available");
        }
    }
}

pub fn pack_ptr(ptr: *const OnsetTableShared) -> (f32, f32) {
    let raw = ptr as usize as u64;
    let lo = raw as u32;
    let hi = (raw >> 32) as u32;
    (f32::from_bits(lo), f32::from_bits(hi))
}

pub fn unpack_ptr(lo: f32, hi: f32) -> *const OnsetTableShared {
    let lo = lo.to_bits() as u64;
    let hi = hi.to_bits() as u64;
    ((hi << 32) | lo) as usize as *const OnsetTableShared
}

pub fn analyze(samples: &[f32], sr: u32, buffer_id: i32) -> Result<AnalysisResult, String> {
    if samples.is_empty() {
        return Err("sample is empty".to_string());
    }
    if sr == 0 {
        return Err("sample rate is zero".to_string());
    }

    #[cfg(not(test))]
    {
        analyze_with_aubio(samples, sr, buffer_id)
    }
    #[cfg(test)]
    {
        Ok(analyze_fallback(samples, sr, buffer_id))
    }
}

#[cfg(not(test))]
fn analyze_with_aubio(samples: &[f32], sr: u32, buffer_id: i32) -> Result<AnalysisResult, String> {
    const BUF_SIZE: usize = 1024;
    const HOP_SIZE: usize = 512;

    let mut tempo = aubio::Tempo::new(aubio::OnsetMode::default(), BUF_SIZE, HOP_SIZE, sr)
        .map_err(|e| format!("aubio tempo init failed: {e:?}"))?;
    let mut onset = aubio::Onset::new(aubio::OnsetMode::Hfc, BUF_SIZE, HOP_SIZE, sr)
        .map_err(|e| format!("aubio onset init failed: {e:?}"))?
        .with_threshold(0.3)
        .with_minioi_ms(40.0);

    let mut onsets_frames = Vec::new();
    let mut hop = [0.0_f32; HOP_SIZE];
    for chunk in samples.chunks(HOP_SIZE) {
        hop.fill(0.0);
        hop[..chunk.len()].copy_from_slice(chunk);
        let _ = tempo.do_result(&hop);
        let onset_value = onset
            .do_result(&hop)
            .map_err(|e| format!("aubio onset failed: {e:?}"))?;
        if onset_value > 0.0 {
            let frame = onset.get_last().min(samples.len()) as u32;
            if onsets_frames.last().copied() != Some(frame) {
                onsets_frames.push(frame);
            }
        }
    }

    if onsets_frames.first().copied().unwrap_or(u32::MAX) != 0 {
        onsets_frames.insert(0, 0);
    }

    let bpm = tempo.get_bpm().max(0.0);
    let confidence = tempo.get_confidence().max(0.0);
    let downbeat_frame = estimate_downbeat(&onsets_frames, bpm, sr);
    Ok(AnalysisResult {
        buffer_id,
        bpm,
        bpm_confidence: confidence,
        onsets_frames,
        downbeat_frame,
    })
}

#[cfg(test)]
fn analyze_fallback(samples: &[f32], sr: u32, buffer_id: i32) -> AnalysisResult {
    let mut onsets = vec![0];
    let min_gap = (sr as f32 * 0.04) as usize;
    let mut last = 0usize;
    for i in 1..samples.len() {
        if i.saturating_sub(last) >= min_gap
            && samples[i].abs() > 0.2
            && samples[i - 1].abs() <= 0.2
        {
            onsets.push(i as u32);
            last = i;
        }
    }
    AnalysisResult {
        buffer_id,
        bpm: 120.0,
        bpm_confidence: 0.5,
        downbeat_frame: onsets.first().copied(),
        onsets_frames: onsets,
    }
}

fn estimate_downbeat(onsets: &[u32], bpm: f32, sr: u32) -> Option<u32> {
    if onsets.is_empty() || bpm <= 0.0 || sr == 0 {
        return onsets.first().copied();
    }
    let beat_frames = sr as f32 * 60.0 / bpm;
    let tolerance = sr as f32 * 0.020;
    onsets
        .iter()
        .copied()
        .find(|frame| {
            let pos = *frame as f32;
            let nearest = (pos / beat_frames).round() * beat_frames;
            (pos - nearest).abs() <= tolerance
        })
        .or_else(|| onsets.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_sensitivity_filters_by_minimum_spacing() {
        let table = SliceTableShared {
            onsets_frames: vec![0, 400, 4_000, 4_400],
            sample_len_frames: 8_000,
            sample_rate: 8_000,
        };
        assert_eq!(table.slice_starts(0.0).collect::<Vec<_>>(), vec![0, 4_000]);
        assert_eq!(
            table.slice_starts(1.0).collect::<Vec<_>>(),
            vec![0, 400, 4_000, 4_400]
        );
    }

    #[test]
    fn pointer_pack_round_trips() {
        let table = OnsetTableShared {
            onsets_frames: vec![0, 128],
            sample_len_frames: 256,
            sample_rate: 44_100,
        };
        let ptr = &table as *const OnsetTableShared;
        let (lo, hi) = pack_ptr(ptr);
        assert_eq!(unpack_ptr(lo, hi), ptr);
    }
}
