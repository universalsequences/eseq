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
    /// Parallel to `onsets_frames`: how hard each onset hits (see
    /// [`onset_strengths`]). Empty means "unknown"; every onset then ranks
    /// as equally strong.
    pub onset_strengths: Vec<f32>,
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
        // `add` refuses a frame that already resolves to a marker, and a move
        // has to refuse it for the same reason: `resolved` dedups, so landing
        // on an existing marker collapses the two. The survivor then answers
        // `delete_index` through the move record, silently destroying the
        // other marker while the delete appears to do nothing.
        if resolved.binary_search(&to).is_ok() {
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

/// Manual overrides are authored against one specific sample. Every consumer
/// resolves them through this filter so a sample swap that never routed
/// through the discard path still cannot apply another sample's markers.
pub fn edits_for_sample<'a>(
    edits: Option<&'a SamplerSliceEdits>,
    sample_path: Option<&str>,
) -> Option<&'a SamplerSliceEdits> {
    let hash = sample_path.and_then(sample_path_hash)?;
    edits.filter(|item| item.sample_hash == hash)
}

/// [`edits_for_sample`] for the sample at `path`.
pub fn edits_for_sample_path<'a>(
    edits: Option<&'a SamplerSliceEdits>,
    path: Option<&std::path::Path>,
) -> Option<&'a SamplerSliceEdits> {
    edits_for_sample(edits, path.map(|path| path.to_string_lossy()).as_deref())
}

/// Stable identity for "the sample these manual slice edits were authored
/// against".
///
/// A content-addressed sample (`samples/<sha256>.wav`) is its own identity, and
/// that spelling is preserved byte-for-byte so projects saved before this
/// function grew a fallback keep matching.
///
/// Everything else falls back to a digest of the path. This is not cosmetic:
/// every consumer treats `None` as "these edits belong to some other sample",
/// so returning `None` for an ordinary `kick.wav` silently discarded the edit
/// on write (`ui/host_commands/sampler_slices.rs`), on read
/// (`edits_for_sample`), on sample rebind (`App::set_sampler_path_for_track`),
/// and on project load (`discard_slice_edits_for_other_sample`) — i.e. manual
/// slice editing did nothing at all unless the sample happened to live in the
/// content-addressed store.
///
/// Path identity is weaker than content identity — editing the file in place
/// keeps the same key — but it is the right granularity for the rule this
/// serves (spec §2.7: *loading a different sample discards the edits*).
pub fn sample_path_hash(path: &str) -> Option<String> {
    let stem = std::path::Path::new(path).file_stem()?.to_str()?;
    if stem.len() == 64 && stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(stem.to_ascii_lowercase());
    }
    if path.is_empty() {
        return None;
    }
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    // Domain-separated so a path digest can never be mistaken for the content
    // hash of a sample in the store.
    hasher.update(b"eseq-sampler-slice-path\0");
    hasher.update(path.as_bytes());
    Some(format!("path:{:x}", hasher.finalize()))
}

#[derive(Clone, Debug)]
pub struct SliceTableShared {
    pub onsets_frames: Vec<u32>,
    /// Parallel to `onsets_frames`: the order in which sensitivity activates
    /// each onset (0 = first to become a slice). Empty falls back to time
    /// order. See [`onset_rank`].
    pub onset_rank: Vec<u32>,
    pub sample_len_frames: u32,
    pub sample_rate: u32,
    pub bpm: f32,
    pub downbeat_frame: Option<u32>,
    pub manual_edits: Option<SamplerSliceEdits>,
}

impl SliceTableShared {
    pub fn from_result(result: &AnalysisResult, sample_len_frames: u32, sample_rate: u32) -> Self {
        Self {
            onsets_frames: result.onsets_frames.clone(),
            onset_rank: onset_rank(
                &result.onsets_frames,
                &result.onset_strengths,
                sample_len_frames,
            ),
            sample_len_frames,
            sample_rate,
            bpm: result.bpm,
            downbeat_frame: result.downbeat_frame,
            manual_edits: None,
        }
    }

    /// Every candidate marker (manual edits applied), paired with whether
    /// sensitivity keeps it as a real slice boundary.
    ///
    /// Sensitivity does not delete markers — it *deactivates* them. Inactive
    /// markers stay visible and stay editable, and playback runs straight
    /// through them, so lowering sensitivity yields longer chops rather than a
    /// marker list that silently shrinks. Marker indices therefore address this
    /// full list and are stable as sensitivity moves.
    pub fn slice_markers(&self, sensitivity: f32) -> (Vec<u32>, Vec<bool>) {
        let all = match self.manual_edits.as_ref() {
            Some(edits) => edits.resolved(&self.onsets_frames, self.sample_len_frames),
            None => {
                let mut all = self
                    .onsets_frames
                    .iter()
                    .copied()
                    .filter(|frame| *frame < self.sample_len_frames)
                    .collect::<Vec<_>>();
                all.push(0);
                all.sort_unstable();
                all.dedup();
                all
            }
        };
        let active = self.slice_starts(sensitivity).collect::<Vec<_>>();
        let flags = all
            .iter()
            .map(|frame| active.binary_search(frame).is_ok())
            .collect();
        (all, flags)
    }

    /// Iterate transient slice starts without allocating on the audio thread.
    ///
    /// Sensitivity spans the whole range from one slice (0) to every detected
    /// onset (1): it activates the `(N + 1)^s - 1` best-ranked onsets, so each
    /// knob step multiplies the slice count rather than adding to it — the
    /// low end yields a few long phrases, the high end every hit. Manual edits
    /// are overlaid afterwards, so authored markers survive every sensitivity
    /// value, including trigger-time parameter locks.
    pub fn slice_starts(&self, sensitivity: f32) -> SliceStarts<'_> {
        let sensitivity = if sensitivity.is_finite() {
            sensitivity.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let candidates = self
            .onsets_frames
            .iter()
            .filter(|frame| is_slice_candidate(**frame, self.sample_len_frames))
            .count();
        SliceStarts {
            table: self,
            active_count: active_onset_count(candidates, sensitivity),
            detected_cursor: 0,
            candidate_ordinal: 0,
            detected_candidate: None,
            last_output: None,
        }
    }

    pub fn with_edits(&self, edits: Option<&SamplerSliceEdits>) -> Self {
        Self {
            onsets_frames: self.onsets_frames.clone(),
            onset_rank: self.onset_rank.clone(),
            sample_len_frames: self.sample_len_frames,
            sample_rate: self.sample_rate,
            bpm: self.bpm,
            downbeat_frame: self.downbeat_frame,
            manual_edits: edits.cloned(),
        }
    }
}

pub struct SliceStarts<'a> {
    table: &'a SliceTableShared,
    /// Onsets ranked below this are active slice boundaries.
    active_count: u32,
    detected_cursor: usize,
    /// In-range onsets seen so far: the rank when the table has no ranking.
    candidate_ordinal: u32,
    detected_candidate: Option<u32>,
    last_output: Option<u32>,
}

impl SliceStarts<'_> {
    fn fill_detected_candidate(&mut self) {
        while self.detected_candidate.is_none() {
            let index = self.detected_cursor;
            let Some(frame) = self.table.onsets_frames.get(index).copied() else {
                return;
            };
            self.detected_cursor += 1;
            if !is_slice_candidate(frame, self.table.sample_len_frames) {
                continue;
            }
            let ordinal = self.candidate_ordinal;
            self.candidate_ordinal += 1;
            let rank = match self.table.onset_rank.get(index) {
                Some(rank) if self.table.onset_rank.len() == self.table.onsets_frames.len() => {
                    *rank
                }
                // No ranking (hand-built tables): activate in time order.
                _ => ordinal,
            };
            if rank >= self.active_count {
                continue;
            }
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

/// How many of `candidates` onsets sensitivity `s` activates:
/// `(candidates + 1)^s - 1`, i.e. one slice at 0 and every onset at 1, with
/// the slice count growing geometrically in between.
fn active_onset_count(candidates: usize, sensitivity: f32) -> u32 {
    if candidates == 0 {
        return 0;
    }
    let slices = ((candidates + 1) as f32).powf(sensitivity);
    (slices.round() as u32).saturating_sub(1).min(candidates as u32)
}

/// Above this many onsets the spread-aware ranking (quadratic) gives way to a
/// plain strength sort; minutes-long material that dense is past the point
/// where the spread term is audible anyway.
const SPREAD_RANK_MAX_ONSETS: usize = 6_000;

/// A detected onset can start a slice only strictly inside the sample: frame
/// 0 is always a slice already. The rank, the candidate count and the
/// iterator must agree on this, so it lives in one place.
fn is_slice_candidate(frame: u32, sample_len_frames: u32) -> bool {
    frame > 0 && frame < sample_len_frames
}

/// The order in which sensitivity activates onsets, parallel to `onsets`.
///
/// Greedy: each pick is the onset that best combines strength with distance
/// from the slices already chosen, measured against the slice length the
/// current count implies. Every prefix of the ranking is therefore a musical
/// slicing on its own — the first few picks land on the big hits *and* divide
/// the sample into phrases, instead of clustering wherever the loudest bar is.
/// Frame 0 and out-of-range onsets rank last (`u32::MAX`).
pub fn onset_rank(onsets: &[u32], strengths: &[f32], sample_len_frames: u32) -> Vec<u32> {
    let mut rank = vec![u32::MAX; onsets.len()];
    let candidates = onsets
        .iter()
        .enumerate()
        .filter(|(_, frame)| is_slice_candidate(**frame, sample_len_frames))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return rank;
    }
    let strength = |index: usize| -> f32 {
        if strengths.len() == onsets.len() {
            let value = strengths[index];
            if value.is_finite() { value.max(0.0) } else { 0.0 }
        } else {
            1.0
        }
    };
    let peak = candidates
        .iter()
        .map(|index| strength(*index))
        .fold(0.0_f32, f32::max);
    // A small floor keeps silent-but-detected onsets orderable by spread.
    let weight = |index: usize| -> f32 {
        if peak > 0.0 { 0.05 + strength(index) / peak } else { 1.0 }
    };

    if candidates.len() > SPREAD_RANK_MAX_ONSETS {
        let mut order = candidates;
        order.sort_by(|a, b| weight(*b).total_cmp(&weight(*a)).then(a.cmp(b)));
        for (position, index) in order.into_iter().enumerate() {
            rank[index] = position as u32;
        }
        return rank;
    }

    // Distance from each candidate to the nearest chosen boundary (frame 0
    // and the sample end count as chosen from the start).
    let len = sample_len_frames as f32;
    let mut nearest = candidates
        .iter()
        .map(|index| {
            let frame = onsets[*index] as f32;
            frame.min(len - frame)
        })
        .collect::<Vec<_>>();
    let mut taken = vec![false; candidates.len()];
    for picked in 0..candidates.len() {
        // The gap one more evenly spread slice would leave.
        let gap = len / (picked + 2) as f32;
        let mut best: Option<(usize, f32)> = None;
        for (slot, index) in candidates.iter().enumerate() {
            if taken[slot] {
                continue;
            }
            let spread = nearest[slot] / (nearest[slot] + 0.5 * gap);
            let score = weight(*index) * spread;
            if best.is_none_or(|(_, top)| score > top) {
                best = Some((slot, score));
            }
        }
        let Some((slot, _)) = best else { break };
        taken[slot] = true;
        rank[candidates[slot]] = picked as u32;
        let frame = onsets[candidates[slot]] as f32;
        for (other, index) in candidates.iter().enumerate() {
            let distance = (onsets[*index] as f32 - frame).abs();
            if distance < nearest[other] {
                nearest[other] = distance;
            }
        }
    }
    rank
}

/// How hard each onset hits: the RMS of the attack window, weighted by how far
/// it rises over the window just before it. A kick out of a quiet gap scores
/// high; a ghost note riding a sustained pad scores low.
pub fn onset_strengths(samples: &[f32], sample_rate: u32, onsets: &[u32]) -> Vec<f32> {
    let window = |ms: f32| ((sample_rate as f32 * ms / 1000.0) as usize).max(1);
    let pre_len = window(30.0);
    let post_len = window(25.0);
    let rms = |from: usize, to: usize| -> f32 {
        let to = to.min(samples.len());
        if from >= to {
            return 0.0;
        }
        let sum = samples[from..to].iter().map(|value| value * value).sum::<f32>();
        (sum / (to - from) as f32).sqrt()
    };
    onsets
        .iter()
        .map(|frame| {
            let frame = *frame as usize;
            let post = rms(frame, frame + post_len);
            let pre = rms(frame.saturating_sub(pre_len), frame);
            let rise = ((post + 1.0e-4) / (pre + 1.0e-4)).log2().clamp(0.25, 6.0);
            post * rise
        })
        .collect()
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
    tx: Option<Sender<AnalysisJob>>,
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
                    Self::analyze_job(&worker_cache, job);
                }
            });
        Self { cache, tx: Some(tx) }
    }

    /// Complete analysis during loading for offline consumers that cannot
    /// depend on UI ticks or background worker timing.
    pub fn synchronous() -> Self {
        Self { cache: AnalysisCache::new(), tx: None }
    }

    fn analyze_job(cache: &AnalysisCache, job: AnalysisJob) {
        let sample_len_frames = job.samples.len() as u32;
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            analyze(&job.samples, job.sample_rate, job.buffer_id)
        }));
        match result {
            Ok(Ok(result)) => cache.insert_ready(
                result,
                sample_len_frames,
                job.sample_rate,
            ),
            Ok(Err(error)) => cache.insert_failed(job.buffer_id, error),
            Err(_) => cache.insert_failed(job.buffer_id, "analysis panicked"),
        }
    }

    /// Require a settled, successful preparation batch before offline playback.
    pub fn require_complete(&self) -> Result<(), String> {
        for (buffer, entry) in self.cache.inner.read().unwrap().iter() {
            match entry.as_ref() {
                AnalysisEntry::Ready(_) => {}
                AnalysisEntry::Pending => return Err(format!("Sample {buffer} analysis is pending")),
                AnalysisEntry::Failed(error) => return Err(format!("Sample {buffer} analysis failed: {error}")),
            }
        }
        Ok(())
    }

    pub fn cache(&self) -> &AnalysisCache {
        &self.cache
    }

    pub fn submit(&self, job: AnalysisJob) {
        self.cache.insert_pending(job.buffer_id);
        let Some(tx) = &self.tx else {
            Self::analyze_job(&self.cache, job);
            return;
        };
        if let Err(error) = tx.send(job) {
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
    let onset_strengths = onset_strengths(samples, sr, &onsets_frames);
    Ok(AnalysisResult {
        buffer_id,
        bpm,
        bpm_confidence: confidence,
        onsets_frames,
        onset_strengths,
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
        onset_strengths: onset_strengths(samples, sr, &onsets),
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
    #[test]
    fn synchronous_analysis_is_ready_before_submit_returns_and_reports_failure() {
        let service = super::AnalysisService::synchronous();
        service.submit(super::AnalysisJob {
            buffer_id: 7, samples: std::sync::Arc::new(vec![0.25; 4096]), sample_rate: 48_000,
        });
        service.require_complete().unwrap();
        assert!(service.cache().table(7).is_some());
        service.submit(super::AnalysisJob {
            buffer_id: 8, samples: std::sync::Arc::new(Vec::new()), sample_rate: 48_000,
        });
        assert!(service.require_complete().unwrap_err().contains("Sample 8 analysis failed"));
    }

    use super::*;

    #[test]
    fn sensitivity_spans_one_slice_to_every_onset() {
        let onsets = vec![0, 400, 4_000, 4_400];
        let table = SliceTableShared {
            onset_rank: onset_rank(&onsets, &[0.0, 0.2, 1.0, 0.3], 8_000),
            onsets_frames: onsets,
            sample_len_frames: 8_000,
            sample_rate: 8_000,
            bpm: 120.0,
            downbeat_frame: Some(0),
            manual_edits: None,
        };
        // Zero is the whole sample as one slice, not "fewer slices".
        assert_eq!(table.slice_starts(0.0).collect::<Vec<_>>(), vec![0]);
        assert_eq!(
            table.slice_starts(1.0).collect::<Vec<_>>(),
            vec![0, 400, 4_000, 4_400]
        );
        // In between, the strongest onset arrives first.
        assert_eq!(table.slice_starts(0.5).collect::<Vec<_>>(), vec![0, 4_000]);
    }

    #[test]
    fn active_onset_count_grows_geometrically_from_one_slice() {
        assert_eq!(active_onset_count(300, 0.0), 0);
        assert_eq!(active_onset_count(300, 1.0), 300);
        // Half way is ~sqrt: a handful of phrases, not half the hits.
        assert_eq!(active_onset_count(300, 0.5), 16);
        assert_eq!(active_onset_count(0, 0.7), 0);
        let counts = (0..=20)
            .map(|step| active_onset_count(300, step as f32 / 20.0))
            .collect::<Vec<_>>();
        assert!(counts.windows(2).all(|pair| pair[0] <= pair[1]), "{counts:?}");
    }

    /// The first picks divide the sample into phrases rather than piling onto
    /// the loudest bar, but a much stronger hit still wins over spread.
    #[test]
    fn onset_rank_balances_strength_against_spread() {
        let len = 100_000;
        // A loud hit plus two ghosts early on, one medium hit mid-sample.
        let onsets = vec![0, 10_000, 12_000, 14_000, 50_000];
        let strengths = vec![0.0, 1.0, 0.3, 0.3, 0.6];
        let rank = onset_rank(&onsets, &strengths, len);
        assert_eq!(rank[0], u32::MAX, "frame 0 is always a boundary, never ranked");
        assert_eq!(rank[4], 0, "the mid-sample hit splits the sample first");
        assert_eq!(rank[1], 1, "then the loud hit, ahead of the better-spaced ghosts");
        // Every candidate gets a distinct rank.
        let mut seen = rank[1..].to_vec();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3]);

        // Without strength information the ranking is pure spread.
        let rank = onset_rank(&onsets, &[], len);
        assert_eq!(rank[4], 0);
    }

    #[test]
    fn onset_strength_rewards_loud_attacks_out_of_quiet() {
        let sr = 1_000;
        let mut samples = vec![0.0_f32; 1_000];
        // A hard hit out of silence at 200, a soft bump on a bed at 600.
        samples[200..260].fill(0.8);
        samples[540..700].fill(0.2);
        samples[600..640].fill(0.3);
        let strengths = onset_strengths(&samples, sr, &[200, 600]);
        assert!(strengths[0] > strengths[1] * 4.0, "{strengths:?}");
    }

    /// Sensitivity deactivates markers rather than removing them, so the
    /// candidate list — and therefore every marker index — is stable as the
    /// knob moves.
    #[test]
    fn sensitivity_deactivates_markers_without_changing_the_candidate_list() {
        let table = SliceTableShared {
            onsets_frames: vec![0, 4_410, 8_820, 44_100, 88_200],
            onset_rank: Vec::new(),
            sample_len_frames: 132_300,
            sample_rate: 44_100,
            bpm: 120.0,
            downbeat_frame: None,
            manual_edits: None,
        };

        let (loose_all, loose_flags) = table.slice_markers(1.0);
        assert_eq!(loose_all, vec![0, 4_410, 8_820, 44_100, 88_200]);
        assert_eq!(loose_flags, vec![true, true, true, true, true]);

        let (tight_all, tight_flags) = table.slice_markers(0.0);
        assert_eq!(
            tight_all, loose_all,
            "the candidate list must not change with sensitivity"
        );
        assert_eq!(tight_flags, vec![true, false, false, false, false]);
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
    fn moving_a_marker_onto_another_is_refused_like_adding_one_there() {
        let mut edits = SamplerSliceEdits::for_sample_hash("abc123");
        let detected = [0, 100, 200, 300];

        // `resolved` dedups, so landing on an existing marker would collapse
        // the two: the survivor answers `delete_index` through the move
        // record, leaving the detected marker in place while the other one is
        // already gone — a delete that appears to do nothing after a silent
        // loss.
        assert!(!edits.move_index(1, 200, &detected, 400));
        assert_eq!(edits.resolved(&detected, 400), vec![0, 100, 200, 300]);

        // Frame 0 is likewise not a landing site, and a legal move still works.
        assert!(!edits.move_index(1, 0, &detected, 400));
        assert!(edits.move_index(1, 150, &detected, 400));
        assert_eq!(edits.resolved(&detected, 400), vec![0, 150, 200, 300]);

        // The moved marker can be moved again, but still not onto a neighbour.
        assert!(!edits.move_index(1, 300, &detected, 400));
        assert_eq!(edits.resolved(&detected, 400), vec![0, 150, 200, 300]);
    }

    #[test]
    fn resolved_manual_table_keeps_user_markers_regardless_of_sensitivity_spacing() {
        let table = SliceTableShared {
            onsets_frames: vec![0, 100, 1_000],
            onset_rank: Vec::new(),
            sample_len_frames: 2_000,
            sample_rate: 1_000,
            bpm: 120.0,
            downbeat_frame: Some(0),
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
            vec![0, 50],
        );
        assert_eq!(
            resolved.slice_starts(1.0).collect::<Vec<_>>(),
            vec![0, 50, 100, 1_000],
        );
    }

    #[test]
    fn edits_for_sample_drops_overrides_authored_against_another_sample() {
        let hash = "a".repeat(64);
        let other = "b".repeat(64);
        let edits = SamplerSliceEdits {
            sample_hash: hash.clone(),
            user_added: vec![50],
            ..SamplerSliceEdits::default()
        };

        assert_eq!(
            edits_for_sample(Some(&edits), Some(&format!("samples/{hash}.wav"))),
            Some(&edits),
        );
        // A sample swap that never routed through a discard path must still not
        // paint the previous sample's markers onto the new one.
        assert_eq!(
            edits_for_sample(Some(&edits), Some(&format!("samples/{other}.wav"))),
            None,
        );
        assert_eq!(edits_for_sample(Some(&edits), Some("samples/kick.wav")), None);
        assert_eq!(edits_for_sample(Some(&edits), None), None);
    }

    /// Regression: manual slice edits were silently discarded on every path
    /// (write, read, rebind, load) for any sample not in the content-addressed
    /// store, which is most of what a user actually drags in.
    #[test]
    fn manual_edits_apply_to_samples_outside_the_content_addressed_store() {
        let path = "/Users/me/Break Squashed 82 bpm.wav";
        let edits = SamplerSliceEdits::for_sample_hash(
            sample_path_hash(path).expect("a plain sample still has an identity"),
        );
        assert!(
            edits_for_sample(Some(&edits), Some(path)).is_some(),
            "edits authored against a plain sample must apply to it"
        );
        assert!(
            edits_for_sample(Some(&edits), Some("/Users/me/Other Break.wav")).is_none(),
            "and must still be dropped for a different sample"
        );
    }

    #[test]
    fn sample_path_hash_accepts_only_content_addressed_references() {
        let hash = "a1".repeat(32);
        assert_eq!(
            sample_path_hash(&format!("samples/{hash}.wav")),
            Some(hash),
        );
        // A sample outside the content-addressed store still gets a stable
        // identity — without one, every manual slice edit is silently dropped.
        let plain = sample_path_hash("samples/kick.wav").expect("plain paths get an identity");
        assert!(plain.starts_with("path:"), "got {plain}");
        assert_eq!(sample_path_hash("samples/kick.wav").as_deref(), Some(plain.as_str()));
        assert_ne!(sample_path_hash("samples/snare.wav"), Some(plain.clone()));
        assert_ne!(
            sample_path_hash("/other/dir/kick.wav"),
            Some(plain),
            "identity is the whole path, not just the file name"
        );
        assert_eq!(sample_path_hash(""), None);
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
            onset_strengths: Vec::new(),
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
            onset_rank: Vec::new(),
            sample_len_frames: 256,
            sample_rate: 44_100,
            bpm: 120.0,
            downbeat_frame: Some(0),
            manual_edits: None,
        };
        let ptr = &table as *const OnsetTableShared;
        let (lo, hi) = pack_ptr(ptr);
        assert_eq!(unpack_ptr(lo, hi), ptr);
    }
}
