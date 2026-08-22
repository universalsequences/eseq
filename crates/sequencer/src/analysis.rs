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

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SamplerSliceEdits {
    /// Content hash from the sample reference these edits belong to. Keeping
    /// it with the edits makes stale overrides impossible to apply after a
    /// sample replacement.
    pub sample_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_added: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_deleted: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_moved: Vec<SliceMove>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SliceMove {
    pub from: u32,
    pub to: u32,
}

impl SamplerSliceEdits {
    pub fn for_sample_hash(sample_hash: impl Into<String>) -> Self {
        Self {
            sample_hash: sample_hash.into(),
            ..Self::default()
        }
    }

    pub fn resolved(&self, detected: &[u32], sample_len_frames: u32) -> Vec<u32> {
        let deleted = |frame: &u32| self.user_deleted.contains(frame)
            || self.user_moved.iter().any(|item| item.from == *frame);
        let mut resolved = detected
            .iter()
            .copied()
            .filter(|frame| !deleted(frame))
            .collect::<Vec<_>>();
        resolved.extend(self.user_added.iter().copied());
        resolved.extend(self.user_moved.iter().map(|item| item.to));
        resolved.retain(|frame| *frame < sample_len_frames);
        resolved.push(0);
        resolved.sort_unstable();
        resolved.dedup();
        resolved
    }

    pub fn add(&mut self, frame: u32, detected: &[u32], sample_len_frames: u32) -> bool {
        if frame == 0
            || frame >= sample_len_frames
            || self
                .resolved(detected, sample_len_frames)
                .binary_search(&frame)
                .is_ok()
        {
            return false;
        }
        self.user_added.push(frame);
        self.normalize();
        true
    }

    pub fn delete_index(
        &mut self,
        index: usize,
        detected: &[u32],
        sample_len_frames: u32,
    ) -> bool {
        let Some(frame) = self
            .resolved(detected, sample_len_frames)
            .get(index)
            .copied()
        else {
            return false;
        };
        if frame == 0 {
            return false;
        }
        if let Some(position) = self.user_added.iter().position(|item| *item == frame) {
            self.user_added.remove(position);
        } else if let Some(position) = self.user_moved.iter().position(|item| item.to == frame) {
            let origin = self.user_moved.remove(position).from;
            self.user_deleted.push(origin);
        } else {
            self.user_deleted.push(frame);
        }
        self.normalize();
        true
    }

    pub fn move_index(
        &mut self,
        index: usize,
        to: u32,
        detected: &[u32],
        sample_len_frames: u32,
    ) -> bool {
        let resolved = self.resolved(detected, sample_len_frames);
        let Some(from) = resolved.get(index).copied() else {
            return false;
        };
        if from == 0 || to == 0 || to >= sample_len_frames || from == to {
            return false;
        }
        if let Some(position) = self.user_added.iter().position(|item| *item == from) {
            self.user_added[position] = to;
        } else if let Some(item) = self.user_moved.iter_mut().find(|item| item.to == from) {
            item.to = to;
        } else {
            self.user_moved.push(SliceMove { from, to });
        }
        self.normalize();
        true
    }

    fn normalize(&mut self) {
        self.user_added.sort_unstable();
        self.user_added.dedup();
        self.user_deleted.sort_unstable();
        self.user_deleted.dedup();
        self.user_moved.sort_unstable_by_key(|item| item.from);
        self.user_moved.dedup_by_key(|item| item.from);
    }
}

pub fn sample_path_hash(path: &str) -> Option<String> {
    let stem = std::path::Path::new(path).file_stem()?.to_str()?;
    (stem.len() == 64 && stem.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| stem.to_ascii_lowercase())
}

#[derive(Clone, Debug)]
pub struct SliceTableShared {
    pub onsets_frames: Vec<u32>,
    pub sample_len_frames: u32,
    pub sample_rate: u32,
    pub manual_edits: Option<SamplerSliceEdits>,
}

impl SliceTableShared {
    pub fn from_result(result: &AnalysisResult, sample_len_frames: u32, sample_rate: u32) -> Self {
        Self {
            onsets_frames: result.onsets_frames.clone(),
            sample_len_frames,
            sample_rate,
            manual_edits: None,
        }
    }

    /// Iterate transient slice starts without allocating on the audio thread.
    /// Aubio has already applied its strength threshold; sensitivity controls
    /// the second half of transient filtering, minimum spacing. Manual edits
    /// are overlaid after that filtering, so authored markers survive every
    /// sensitivity value, including trigger-time parameter locks.
    pub fn slice_starts(&self, sensitivity: f32) -> SliceStarts<'_> {
        let sensitivity = if sensitivity.is_finite() {
            sensitivity.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let min_spacing_ms = 500.0 + (40.0 - 500.0) * sensitivity;
        SliceStarts {
            table: self,
            min_spacing: (self.sample_rate as f32 * min_spacing_ms / 1000.0).round() as u32,
            detected_cursor: 0,
            last_detected: 0,
            detected_candidate: None,
            last_output: None,
        }
    }

    pub fn with_edits(&self, edits: Option<&SamplerSliceEdits>) -> Self {
        Self {
            onsets_frames: self.onsets_frames.clone(),
            sample_len_frames: self.sample_len_frames,
            sample_rate: self.sample_rate,
            manual_edits: edits.cloned(),
        }
    }
}

pub struct SliceStarts<'a> {
    table: &'a SliceTableShared,
    min_spacing: u32,
    detected_cursor: usize,
    last_detected: u32,
    detected_candidate: Option<u32>,
    last_output: Option<u32>,
}

impl SliceStarts<'_> {
    fn fill_detected_candidate(&mut self) {
        while self.detected_candidate.is_none() {
            let Some(frame) = self.table.onsets_frames.get(self.detected_cursor).copied() else {
                return;
            };
            self.detected_cursor += 1;
            if frame == 0 || frame >= self.table.sample_len_frames {
                continue;
            }
            if frame.saturating_sub(self.last_detected) < self.min_spacing {
                continue;
            }
            self.last_detected = frame;
            let suppressed = self.table.manual_edits.as_ref().is_some_and(|edits| {
                edits.user_deleted.contains(&frame)
                    || edits.user_moved.iter().any(|item| item.from == frame)
            });
            if !suppressed {
                self.detected_candidate = Some(frame);
            }
        }
    }

    fn next_manual_candidate(&self) -> Option<u32> {
        let edits = self.table.manual_edits.as_ref()?;
        let after = self.last_output.unwrap_or(0);
        edits
            .user_added
            .iter()
            .copied()
            .chain(edits.user_moved.iter().map(|item| item.to))
            .filter(|frame| *frame > after && *frame < self.table.sample_len_frames)
            .min()
    }
}

impl Iterator for SliceStarts<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.last_output.is_none() {
            self.last_output = Some(0);
            return Some(0);
        }
        self.fill_detected_candidate();
        let manual = self.next_manual_candidate();
        let next = match (self.detected_candidate, manual) {
            (Some(detected), Some(manual)) => detected.min(manual),
            (Some(detected), None) => detected,
            (None, Some(manual)) => manual,
            (None, None) => return None,
        };
        if self.detected_candidate == Some(next) {
            self.detected_candidate = None;
        }
        self.last_output = Some(next);
        Some(next)
    }
}

/// Warp and slice resolution intentionally share the same immutable analysis table.
pub type OnsetTableShared = SliceTableShared;

#[derive(Clone)]
struct PublishedSliceTable {
    buffer_id: i32,
    edits: SamplerSliceEdits,
    table: Arc<OnsetTableShared>,
}

#[derive(Clone, Default)]
pub struct AnalysisCache {
    inner: Arc<RwLock<HashMap<i32, Arc<AnalysisEntry>>>>,
    tables: Arc<RwLock<HashMap<i32, Arc<OnsetTableShared>>>>,
    published_slice_tables: Arc<RwLock<HashMap<usize, PublishedSliceTable>>>,
    /// Tables displaced by a re-analysis of the same buffer id. The audio
    /// thread reads published tables through a raw pointer (`pack_ptr`), so a
    /// table that was ever published must stay allocated for the app lifetime:
    /// buffer ids are recycled by the audio graph, and freeing the old table on
    /// replacement would leave a pool pointing at freed memory until the UI
    /// thread republishes. Manual marker drags publish only on gesture commit,
    /// so retention grows with completed edits and sample loads rather than
    /// pointer-move events; each table is a small onset vector plus its edits.
    retired_tables: Arc<RwLock<Vec<Arc<OnsetTableShared>>>>,
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
        if let Some(displaced) = self.tables.write().unwrap().insert(buffer_id, table) {
            self.retired_tables.write().unwrap().push(displaced);
        }
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

    pub fn table_for_pool(
        &self,
        pool: usize,
        buffer_id: i32,
        edits: Option<&SamplerSliceEdits>,
    ) -> Option<Arc<OnsetTableShared>> {
        let base = self.table(buffer_id)?;
        let Some(edits) = edits else { return Some(base) };
        if let Some(existing) = self.published_slice_tables.read().unwrap().get(&pool) {
            if existing.buffer_id == buffer_id && existing.edits == *edits {
                return Some(existing.table.clone());
            }
        }
        let table = Arc::new(base.with_edits(Some(edits)));
        let published = PublishedSliceTable {
            buffer_id,
            edits: edits.clone(),
            table: table.clone(),
        };
        if let Some(displaced) = self
            .published_slice_tables
            .write()
            .unwrap()
            .insert(pool, published)
        {
            self.retired_tables.write().unwrap().push(displaced.table);
        }
        Some(table)
    }

    #[cfg(test)]
    pub(crate) fn retired_table_count(&self) -> usize {
        self.retired_tables.read().unwrap().len()
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
            manual_edits: None,
        };
        assert_eq!(table.slice_starts(0.0).collect::<Vec<_>>(), vec![0, 4_000]);
        assert_eq!(
            table.slice_starts(1.0).collect::<Vec<_>>(),
            vec![0, 400, 4_000, 4_400]
        );
    }

    #[test]
    fn manual_slice_edits_resolve_sorted_deduped_and_keep_frame_zero() {
        let mut edits = SamplerSliceEdits::for_sample_hash("abc123");
        let detected = [0, 100, 200, 300];
        assert!(edits.add(150, &detected, 400));
        assert!(edits.move_index(2, 175, &detected, 400));
        assert!(edits.delete_index(3, &detected, 400));
        assert_eq!(edits.resolved(&detected, 400), vec![0, 100, 175, 300]);
        assert!(!edits.delete_index(0, &detected, 400));
        assert_eq!(edits.resolved(&detected, 400)[0], 0);
    }

    #[test]
    fn resolved_manual_table_keeps_user_markers_regardless_of_sensitivity_spacing() {
        let table = SliceTableShared {
            onsets_frames: vec![0, 100, 1_000],
            sample_len_frames: 2_000,
            sample_rate: 1_000,
            manual_edits: None,
        };
        let edits = SamplerSliceEdits {
            sample_hash: "a".repeat(64),
            user_added: vec![50],
            ..SamplerSliceEdits::default()
        };

        let resolved = table.with_edits(Some(&edits));

        assert_eq!(
            resolved.slice_starts(0.0).collect::<Vec<_>>(),
            vec![0, 50, 1_000],
        );
        assert_eq!(
            resolved.slice_starts(1.0).collect::<Vec<_>>(),
            vec![0, 50, 100, 1_000],
        );
    }

    #[test]
    fn sample_path_hash_accepts_only_content_addressed_references() {
        let hash = "a1".repeat(32);
        assert_eq!(
            sample_path_hash(&format!("samples/{hash}.wav")),
            Some(hash),
        );
        assert_eq!(sample_path_hash("samples/kick.wav"), None);
    }

    #[test]
    fn reanalyzing_a_recycled_buffer_id_keeps_the_published_table_alive() {
        // The audio thread holds published tables as raw pointers, so a table
        // displaced by a later analysis of the same (recycled) buffer id must
        // not be freed.
        let cache = AnalysisCache::new();
        let result = AnalysisResult {
            buffer_id: 7,
            bpm: 120.0,
            bpm_confidence: 1.0,
            onsets_frames: vec![0, 128],
            downbeat_frame: None,
        };
        cache.insert_ready(result.clone(), 256, 44_100);
        let first = cache.table(7).expect("table published");
        let first_ptr = Arc::as_ptr(&first);
        drop(first);

        cache.insert_ready(result, 512, 48_000);
        assert_eq!(cache.retired_table_count(), 1);
        // Safe: the retired list keeps the original allocation alive.
        let retained = unsafe { &*first_ptr };
        assert_eq!(retained.sample_len_frames, 256);
        assert_eq!(cache.table(7).unwrap().sample_len_frames, 512);
    }

    #[test]
    fn pointer_pack_round_trips() {
        let table = OnsetTableShared {
            onsets_frames: vec![0, 128],
            sample_len_frames: 256,
            sample_rate: 44_100,
            manual_edits: None,
        };
        let ptr = &table as *const OnsetTableShared;
        let (lo, hi) = pack_ptr(ptr);
        assert_eq!(unpack_ptr(lo, hi), ptr);
    }
}
