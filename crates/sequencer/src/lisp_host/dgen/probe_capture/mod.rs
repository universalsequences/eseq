/*!
Audio-thread capture of DGen `(probe …)` taps (docs/patcher-probes-spec.md §5).

A compiled source with probes writes each probe's signal to a hidden output
channel (`DGenManifest::probes`). After a block renders, the instrument and
effect wrappers hand those buffers to this module, which reduces each probe
channel to a per-block summary (`last`/`min`/`max`) and, for `@view scope`,
appends a decimated min/max trace to a fixed ring. Nothing here allocates on
the audio thread.

# Instances

A [`ProbeSet`] holds the preallocated slots of one live DGen instance. It is
built on the main thread when the instance is built and published through an
`AtomicPtr` table, one table per instance kind:

- **Instruments** are keyed by **engine id**
  ([`ProbeInstance::Instrument`]). The set is published where the engine's
  voice registry is (`set_dgen_instrument_amp_channel`'s sites in
  `app/graph/node_build.rs` and `app/graph/engine_connect.rs`). Every voice
  runs the probes, but only the engine's *display voice*, the voice the
  engine pool most recently allocated (`note_dgen_display_voice`, called from
  `ActiveVoicePool::note_voice_allocated`), feeds the capture.
- **Effects** are keyed by their audiograph **node id**
  ([`ProbeInstance::Effect`]). Effect state carries no stable identity the
  wrapper can map to a chain slot (`state[0]` is a diagnostics slot id that
  goes stale when a chain shifts), so `add_effect_to_chain_at_successor`
  allocates a *probe token* for an effect whose manifest has probes and
  writes it into `state[0]` as a negative *probe code*
  ([`encode_effect_probe_token`]; see the header layout in `dgen_ffi.rs` for
  why no dedicated header slot exists). The wrapper reads that plain state
  float: an effect without probes pays no atomic load at all. The main thread
  maps node id → token; the per-node teardown hook
  (`effects::dgen_builtin::clear_instance`) releases it.

  Token reuse is safe because the code also carries the token's
  *generation*. Teardown only queues the node's deletion, so the old node can
  render a few more blocks after its token is released and handed to a new
  effect. Every allocation bumps the token's generation and stamps it on the
  new set; the wrapper captures only when the generation in its own code
  matches the published set's, so a stale node never writes into its
  successor's slots, whatever the timing (no quarantine to tune). The same
  `process_fn` (two instances of one effect) is exactly the case the
  process-function identity check cannot catch.

# Resolving the instance a patcher edits (slice .4)

- Instrument patcher: the track's custom instrument engine id, the same
  `engine_id` that indexes `app.graph.engine_node_ids` (for a rack slot, the
  slot's engine). `ProbeInstance::Instrument { engine_id }`.
- Effect patcher: the fx chain slot the patcher was opened from
  (`FxChainLocator` + slot index) resolves to the live `node_id` in the chain
  host's slots (`slot.node_id`). `ProbeInstance::Effect { node_id }`. The node
  id changes on every recompile (the node is replaced), so resolve it per
  publish, not once.
- No live instance (a patcher opened on a file) shows probes as `—`:
  [`probe_snapshot`] returns `None`.

# Lifetime

The audio thread never owns a reference count. Main-thread code owns every
set through the registry mirror (`ProbeRegistry`, behind a mutex) and
publishes `Arc::as_ptr` to the audio-visible table. Replacing or clearing a
set moves the old `Arc` to a retired list. An audio-thread reader raises
`PROBE_READERS` before it loads a pointer and lowers it when done; the main
thread frees retired sets only after it observes `PROBE_READERS == 0`
following the swap, so no reader can still hold one. A stale node still
rendering after its replacement is caught by the `process_fn` identity check:
a set captures only from the compiled library it was built for.

# Gate

`PROBES_WATCHED` counts open watchers (the UI raises it while a patcher with
live probes is visible). At zero every wrapper returns after one relaxed load.
*/

use std::collections::HashMap;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio::MAX_VOICES;
use crate::sequencer::MAX_INSTRUMENT_ENGINES;

use eseqlisp::live_audio::ProbeView;

use super::dgen_manifest::{DGenManifest, DGenProbe};

/// Min/max pairs each block contributes to a scope ring. The block is split
/// into this many equal chunks (fewer when the block is shorter), so peaks
/// survive decimation.
pub const PROBE_SCOPE_POINTS_PER_BLOCK: usize = 8;
/// Scope ring capacity in min/max pairs: 128 blocks, which is ~340 ms at
/// 128-frame blocks and 48 kHz. Power of two so the index wraps with a mask.
/// A read returns at most `PROBE_SCOPE_RING_PAIRS -
/// PROBE_SCOPE_POINTS_PER_BLOCK` pairs: the oldest block's slots may be
/// mid-rewrite.
pub const PROBE_SCOPE_RING_PAIRS: usize = 1024;
/// Live effects with probes at once. Beyond this an effect still renders but
/// captures nothing.
pub const MAX_EFFECT_PROBE_INSTANCES: usize = 256;

/// Number of open probe watchers; capture runs only while it is nonzero.
static PROBES_WATCHED: AtomicUsize = AtomicUsize::new(0);
/// Audio-thread readers currently dereferencing a published set.
static PROBE_READERS: AtomicUsize = AtomicUsize::new(0);

static INSTRUMENT_PROBE_SETS: [AtomicPtr<ProbeSet>; MAX_INSTRUMENT_ENGINES] =
    [const { AtomicPtr::new(std::ptr::null_mut()) }; MAX_INSTRUMENT_ENGINES];
static EFFECT_PROBE_SETS: [AtomicPtr<ProbeSet>; MAX_EFFECT_PROBE_INSTANCES] =
    [const { AtomicPtr::new(std::ptr::null_mut()) }; MAX_EFFECT_PROBE_INSTANCES];
/// Voice index whose block feeds an engine's probe capture: the voice the
/// engine pool most recently allocated.
static DISPLAY_VOICES: [AtomicUsize; MAX_INSTRUMENT_ENGINES] =
    [const { AtomicUsize::new(0) }; MAX_INSTRUMENT_ENGINES];

// ── Gate ──

/// Raise the watch count; capture runs while any watcher is open. Pair with
/// [`unwatch_probes`], or use [`ProbeWatchGuard`].
pub fn watch_probes() {
    PROBES_WATCHED.fetch_add(1, Ordering::AcqRel);
}

/// Lower the watch count (saturating at zero).
pub fn unwatch_probes() {
    let _ = PROBES_WATCHED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        Some(count.saturating_sub(1))
    });
}

pub fn probes_watched() -> bool {
    PROBES_WATCHED.load(Ordering::Acquire) != 0
}

/// Holds the capture gate open for its lifetime.
#[must_use = "capture stops when the guard drops"]
pub struct ProbeWatchGuard(());

impl ProbeWatchGuard {
    pub fn new() -> Self {
        watch_probes();
        Self(())
    }
}

impl Drop for ProbeWatchGuard {
    fn drop(&mut self) {
        unwatch_probes();
    }
}

// ── Display voice ──

/// Record that the engine pool just allocated `voice_idx`. Audio thread.
#[inline]
pub fn note_dgen_display_voice(engine_id: usize, voice_idx: usize) {
    if engine_id < MAX_INSTRUMENT_ENGINES && voice_idx < MAX_VOICES {
        DISPLAY_VOICES[engine_id].store(voice_idx, Ordering::Relaxed);
    }
}

pub fn dgen_display_voice(engine_id: usize) -> Option<usize> {
    (engine_id < MAX_INSTRUMENT_ENGINES).then(|| DISPLAY_VOICES[engine_id].load(Ordering::Relaxed))
}

// ── Probe slots ──

/// A decimated min/max trace written by one producer (the wrapper rendering
/// the instance) and read by any thread.
struct ScopeRing {
    mins: Box<[AtomicU32]>,
    maxs: Box<[AtomicU32]>,
    /// Pairs ever written; the next write lands at `written % capacity`.
    written: AtomicU64,
}

impl ScopeRing {
    fn new() -> Self {
        Self {
            mins: (0..PROBE_SCOPE_RING_PAIRS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            maxs: (0..PROBE_SCOPE_RING_PAIRS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            written: AtomicU64::new(0),
        }
    }

    fn push_block(&self, samples: &[f32]) {
        let points = PROBE_SCOPE_POINTS_PER_BLOCK.min(samples.len());
        if points == 0 {
            return;
        }
        let start = self.written.load(Ordering::Relaxed);
        for point in 0..points {
            let lo = point * samples.len() / points;
            let hi = (point + 1) * samples.len() / points;
            let (min, max) = min_max(&samples[lo..hi]);
            let index = (start as usize + point) & (PROBE_SCOPE_RING_PAIRS - 1);
            self.mins[index].store(min.to_bits(), Ordering::Relaxed);
            self.maxs[index].store(max.to_bits(), Ordering::Relaxed);
        }
        self.written.store(start + points as u64, Ordering::Release);
    }

    /// The most recent `max_pairs` pairs, oldest first. Pairs the producer
    /// may have overwritten while they were copied are dropped from the
    /// front.
    ///
    /// The producer writes up to [`PROBE_SCOPE_POINTS_PER_BLOCK`] pairs at
    /// `written..` *before* it publishes the new `written`, so when the
    /// reader sees `after` the producer may already be rewriting the slots
    /// of pairs `after + POINTS - RING ..`; only pairs at or after that are
    /// intact. A pair's min and max are two separate atomics, but both are
    /// written before `written` advances past it, so an intact pair is never
    /// torn. (The slot's `last`/`min`/`max` summary, by contrast, may mix two
    /// blocks; it is display-only.)
    fn read_recent(&self, max_pairs: usize, out: &mut Vec<(f32, f32)>) {
        out.clear();
        let end = self.written.load(Ordering::Acquire);
        let wanted = (max_pairs.min(PROBE_SCOPE_RING_PAIRS) as u64).min(end);
        let start = end - wanted;
        for pair in start..end {
            let index = pair as usize & (PROBE_SCOPE_RING_PAIRS - 1);
            out.push((
                f32::from_bits(self.mins[index].load(Ordering::Relaxed)),
                f32::from_bits(self.maxs[index].load(Ordering::Relaxed)),
            ));
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let after = self.written.load(Ordering::Relaxed);
        let oldest_intact = (after + PROBE_SCOPE_POINTS_PER_BLOCK as u64)
            .saturating_sub(PROBE_SCOPE_RING_PAIRS as u64);
        if oldest_intact > start {
            let torn = ((oldest_intact - start) as usize).min(out.len());
            out.drain(..torn);
        }
    }
}

fn min_max(samples: &[f32]) -> (f32, f32) {
    samples
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), &sample| {
            (min.min(sample), max.max(sample))
        })
}

struct ProbeSlot {
    meta: DGenProbe,
    last: AtomicU32,
    min: AtomicU32,
    max: AtomicU32,
    /// Blocks captured; 0 until the first.
    seq: AtomicU64,
    scope: Option<ScopeRing>,
}

impl ProbeSlot {
    /// Reduce one block. The summary's three stores are independent relaxed
    /// atomics, so a concurrent reader may see `last`/`min`/`max` from two
    /// different blocks; that is fine for a display value.
    fn capture(&self, samples: &[f32]) {
        let Some(&last) = samples.last() else {
            return;
        };
        let (min, max) = min_max(samples);
        self.last.store(last.to_bits(), Ordering::Relaxed);
        self.min.store(min.to_bits(), Ordering::Relaxed);
        self.max.store(max.to_bits(), Ordering::Relaxed);
        if let Some(scope) = &self.scope {
            scope.push_block(samples);
        }
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq + 1, Ordering::Release);
    }
}

/// The probe slots of one live DGen instance, built from its manifest.
pub struct ProbeSet {
    /// Address of the compiled `process_fn` the set belongs to.
    process_fn: usize,
    /// Effects: the token generation this set was allocated under (see
    /// [`encode_effect_probe_token`]). Instruments: 0, never checked.
    generation: u32,
    slots: Box<[ProbeSlot]>,
}

impl ProbeSet {
    /// `None` when the manifest declares no addressable probe (non-empty
    /// id) on a channel the instance has a buffer for. An id-less entry
    /// stays in the manifest, so its channel is still kept out of the audio
    /// routes, but nothing could look its value up.
    fn from_manifest(manifest: &DGenManifest, process_fn: usize, generation: u32) -> Option<Arc<Self>> {
        let output_count = manifest.n_outputs.max(1);
        let slots: Box<[ProbeSlot]> = manifest
            .probes
            .iter()
            .filter(|probe| probe.channel < output_count && !probe.id.is_empty())
            .map(|probe| ProbeSlot {
                meta: probe.clone(),
                last: AtomicU32::new(0),
                min: AtomicU32::new(0),
                max: AtomicU32::new(0),
                seq: AtomicU64::new(0),
                scope: (probe.view == ProbeView::Scope).then(ScopeRing::new),
            })
            .collect();
        (!slots.is_empty()).then(|| Arc::new(Self { process_fn, generation, slots }))
    }

    /// Reduce one rendered block. `out` holds the instance's output buffers;
    /// every probe channel is below the manifest's output count, which sized
    /// them.
    ///
    /// # Safety
    /// `out` must point at the output buffers of an instance compiled from the
    /// library this set was built for, each valid for `nframes` floats.
    unsafe fn capture(&self, out: *const *mut f32, nframes: usize) {
        if out.is_null() || nframes == 0 {
            return;
        }
        for slot in self.slots.iter() {
            let channel = *out.add(slot.meta.channel);
            if channel.is_null() {
                continue;
            }
            slot.capture(std::slice::from_raw_parts(channel, nframes));
        }
    }

    fn slot(&self, id: &str, occurrence: u32) -> Option<&ProbeSlot> {
        self.slots
            .iter()
            .find(|slot| slot.meta.id == id && slot.meta.occurrence == occurrence)
    }
}

// ── Audio-thread capture ──

struct ReaderGuard;

impl ReaderGuard {
    #[inline]
    fn enter() -> Self {
        PROBE_READERS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ReaderGuard {
    #[inline]
    fn drop(&mut self) {
        PROBE_READERS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[inline]
unsafe fn capture_published(
    cell: &AtomicPtr<ProbeSet>,
    process_fn: usize,
    generation: Option<u32>,
    out: *const *mut f32,
    nframes: usize,
) {
    // Cheap miss first: an instance without probes never touches the
    // reader count.
    if cell.load(Ordering::Relaxed).is_null() {
        return;
    }
    let _reader = ReaderGuard::enter();
    let set = cell.load(Ordering::SeqCst);
    if set.is_null() {
        return;
    }
    let set = &*set;
    if set.process_fn == process_fn && generation.is_none_or(|generation| generation == set.generation) {
        set.capture(out, nframes);
    }
}

/// Capture an instrument voice's probes after it rendered a block. Only the
/// engine's display voice feeds capture. Audio thread.
///
/// # Safety
/// `out` must be the voice node's output buffers, valid for `nframes`, as
/// rendered by `process_fn`.
#[inline]
pub(in crate::lisp_host) unsafe fn record_dgen_voice_probes(
    engine_id: usize,
    voice_idx: usize,
    process_fn: usize,
    out: *const *mut f32,
    nframes: i32,
) {
    if PROBES_WATCHED.load(Ordering::Relaxed) == 0 {
        return;
    }
    if engine_id >= MAX_INSTRUMENT_ENGINES
        || nframes <= 0
        || DISPLAY_VOICES[engine_id].load(Ordering::Relaxed) != voice_idx
    {
        return;
    }
    capture_published(
        &INSTRUMENT_PROBE_SETS[engine_id],
        process_fn,
        None,
        out,
        nframes as usize,
    );
}

/// Generations a token cycles through before one repeats. With
/// [`MAX_EFFECT_PROBE_INSTANCES`] tokens the largest code magnitude is
/// `256 * 65536 = 2^24`, the last integer an `f32` holds exactly.
const EFFECT_PROBE_GENERATIONS: u32 = 1 << 16;
const _: () = assert!(
    MAX_EFFECT_PROBE_INSTANCES as u64 * EFFECT_PROBE_GENERATIONS as u64 <= 1 << 24,
    "effect probe codes must stay exact in an f32"
);

/// `state[0]` value marking an effect that owns probe `token` under
/// `generation`: `-(1 + token + MAX_EFFECT_PROBE_INSTANCES * generation)`.
/// Negative, so it never collides with a (non-negative) diagnostics slot id.
pub fn encode_effect_probe_token(token: usize, generation: u32) -> f32 {
    let generation = generation % EFFECT_PROBE_GENERATIONS;
    -((1 + token + MAX_EFFECT_PROBE_INSTANCES * generation as usize) as f32)
}

/// The `(token, generation)` a `state[0]` value carries, if any.
#[inline]
pub fn decode_effect_probe_token(header: f32) -> Option<(usize, u32)> {
    if !(header <= -1.0) {
        return None;
    }
    let code = (-header) as usize - 1;
    Some((
        code % MAX_EFFECT_PROBE_INSTANCES,
        (code / MAX_EFFECT_PROBE_INSTANCES) as u32,
    ))
}

/// Capture an effect's probes after it rendered a block. Audio thread.
///
/// # Safety
/// `out` must be the effect node's output buffers, valid for `nframes`, as
/// rendered by `process_fn`.
#[inline]
pub(in crate::lisp_host) unsafe fn record_dgen_effect_probes(
    header: f32,
    process_fn: usize,
    out: *const *mut f32,
    nframes: i32,
) {
    let Some((token, generation)) = decode_effect_probe_token(header) else {
        return;
    };
    if PROBES_WATCHED.load(Ordering::Relaxed) == 0 || nframes <= 0 {
        return;
    }
    if let Some(cell) = EFFECT_PROBE_SETS.get(token) {
        capture_published(cell, process_fn, Some(generation), out, nframes as usize);
    }
}

// ── Main-thread registry ──

#[derive(Default)]
struct ProbeRegistry {
    instruments: HashMap<usize, Arc<ProbeSet>>,
    /// Token → set, for live effects with probes.
    effects: HashMap<usize, Arc<ProbeSet>>,
    effect_nodes: HashMap<i32, usize>,
    /// Tokens allocated but not yet bound to a node id.
    pending_tokens: Vec<usize>,
    /// Allocations per token so far; the next allocation's generation.
    token_generations: HashMap<usize, u32>,
    retired: Vec<Arc<ProbeSet>>,
}

impl ProbeRegistry {
    fn retire(&mut self, set: Option<Arc<ProbeSet>>) {
        self.retired.extend(set);
        self.reclaim();
    }

    /// Free retired sets once no audio-thread reader can hold one. Every
    /// retired set was unpublished before this check, so a reader that
    /// starts after it sees the replacement.
    fn reclaim(&mut self) {
        if !self.retired.is_empty() && PROBE_READERS.load(Ordering::SeqCst) == 0 {
            self.retired.clear();
        }
    }

    fn token_in_use(&self, token: usize) -> bool {
        self.effects.contains_key(&token) || self.pending_tokens.contains(&token)
    }
}

fn registry() -> std::sync::MutexGuard<'static, ProbeRegistry> {
    static REGISTRY: OnceLock<Mutex<ProbeRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn publish(cell: &AtomicPtr<ProbeSet>, set: Option<&Arc<ProbeSet>>) {
    let ptr = set.map_or(std::ptr::null_mut(), |set| Arc::as_ptr(set).cast_mut());
    cell.store(ptr, Ordering::SeqCst);
}

/// Publish (or clear, for a manifest without probes) an instrument engine's
/// probe set. Main thread, wherever the engine's voice registry is published.
pub fn publish_dgen_instrument_probes(
    engine_id: usize,
    manifest: &DGenManifest,
    process_fn: usize,
) {
    if engine_id >= MAX_INSTRUMENT_ENGINES {
        return;
    }
    let set = ProbeSet::from_manifest(manifest, process_fn, 0);
    let mut registry = registry();
    publish(&INSTRUMENT_PROBE_SETS[engine_id], set.as_ref());
    let old = match set {
        Some(set) => registry.instruments.insert(engine_id, set),
        None => registry.instruments.remove(&engine_id),
    };
    registry.retire(old);
}

/// Forget a deleted engine's probes. Main thread, from engine teardown, so
/// a dead engine never reports probes to a reader.
pub fn clear_dgen_instrument_probes(engine_id: usize) {
    if engine_id >= MAX_INSTRUMENT_ENGINES {
        return;
    }
    let mut registry = registry();
    publish(&INSTRUMENT_PROBE_SETS[engine_id], None);
    let old = registry.instruments.remove(&engine_id);
    registry.retire(old);
}

/// A probe token handed to an effect about to be built, with the `state[0]`
/// code that names it ([`encode_effect_probe_token`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectProbeToken {
    pub token: usize,
    pub generation: u32,
}

impl EffectProbeToken {
    /// The value to write into the node's `state[0]`.
    pub fn header_code(self) -> f32 {
        encode_effect_probe_token(self.token, self.generation)
    }
}

/// Allocate a probe token for an effect about to be built from `manifest`
/// and publish its set. `None` when it has no probes or every token is in
/// use; the effect then keeps its slot id in `state[0]`. Bind the token to
/// the node with [`bind_effect_probe_node`], or give it back with
/// [`release_effect_probe_token`] if the node is never built.
///
/// A released token is reusable at once: the new allocation gets the next
/// generation, which a stale node still carrying the old code cannot match.
pub fn register_effect_probes(manifest: &DGenManifest, process_fn: usize) -> Option<EffectProbeToken> {
    let mut registry = registry();
    let token = (0..MAX_EFFECT_PROBE_INSTANCES).find(|&token| !registry.token_in_use(token))?;
    let counter = registry.token_generations.entry(token).or_insert(0);
    let generation = *counter % EFFECT_PROBE_GENERATIONS;
    let set = ProbeSet::from_manifest(manifest, process_fn, generation)?;
    *counter = counter.wrapping_add(1);
    publish(&EFFECT_PROBE_SETS[token], Some(&set));
    registry.effects.insert(token, set);
    registry.pending_tokens.push(token);
    Some(EffectProbeToken { token, generation })
}

/// Record that `node_id` is the effect built with `token`.
pub fn bind_effect_probe_node(token: EffectProbeToken, node_id: i32) {
    let token = token.token;
    let mut registry = registry();
    registry.pending_tokens.retain(|&pending| pending != token);
    if let Some(previous) = registry.effect_nodes.insert(node_id, token) {
        if previous != token {
            release_token(&mut registry, previous);
        }
    }
}

/// Give back a token whose node was never built.
pub fn release_effect_probe_token(token: EffectProbeToken) {
    let token = token.token;
    let mut registry = registry();
    registry.pending_tokens.retain(|&pending| pending != token);
    release_token(&mut registry, token);
}

/// Forget the probes of a deleted effect node. Main thread, from the
/// per-node teardown hook.
pub fn clear_effect_probes(node_id: i32) {
    let mut registry = registry();
    if let Some(token) = registry.effect_nodes.remove(&node_id) {
        release_token(&mut registry, token);
    }
}

fn release_token(registry: &mut ProbeRegistry, token: usize) {
    if let Some(cell) = EFFECT_PROBE_SETS.get(token) {
        publish(cell, None);
    }
    let old = registry.effects.remove(&token);
    registry.retire(old);
}

/// Free sets retired by earlier rebuilds if the audio thread is not reading.
/// Every publish and read already tries; this is for idle callers.
pub fn reclaim_retired_probe_sets() {
    registry().reclaim();
}

// ── Main-thread reads (slice .4) ──

/// A live DGen instance whose probes can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProbeInstance {
    /// A custom instrument engine; its display voice feeds capture.
    Instrument { engine_id: usize },
    /// A DGen effect node in a track, bus or rack-slot chain.
    Effect { node_id: i32 },
}

/// One probe's latest block summary. `seq` counts captured blocks; it stays
/// put while the gate is closed, the display voice is idle, or the effect is
/// bypassed, so a reader can tell a stale value from a static one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeReading {
    pub last: f32,
    pub min: f32,
    pub max: f32,
    pub seq: u64,
}

fn instance_set(instance: ProbeInstance) -> Option<Arc<ProbeSet>> {
    let mut registry = registry();
    registry.reclaim();
    match instance {
        ProbeInstance::Instrument { engine_id } => registry.instruments.get(&engine_id).cloned(),
        ProbeInstance::Effect { node_id } => {
            let token = *registry.effect_nodes.get(&node_id)?;
            registry.effects.get(&token).cloned()
        }
    }
}

/// A main-thread handle on the probe set an instance has right now: one
/// registry lock to get it, then any number of slot reads without locking.
/// Holding it keeps the set's memory alive; it does not keep the set
/// published (a rebuild publishes a new set, which a fresh
/// [`probe_set`] call returns).
#[derive(Clone)]
pub struct ProbeSetHandle(Arc<ProbeSet>);

impl ProbeSetHandle {
    /// Whether both handles name the same set (the same build of the
    /// instance): slot indices and metadata are then interchangeable.
    pub fn same_set(&self, other: &ProbeSetHandle) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn len(&self) -> usize {
        self.0.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.slots.is_empty()
    }

    /// The probes, in manifest order (id-less entries skipped).
    pub fn probes(&self) -> impl ExactSizeIterator<Item = &DGenProbe> + '_ {
        self.0.slots.iter().map(|slot| &slot.meta)
    }

    /// The latest block summary of slot `index`. `None` when out of range or
    /// no block has been captured yet.
    pub fn reading(&self, index: usize) -> Option<ProbeReading> {
        self.0.slots.get(index).and_then(slot_reading)
    }

    /// Fill `out` with the most recent `max_pairs` scope pairs of slot
    /// `index`, oldest first. `false` (and `out` empty) when the slot is out
    /// of range or not a `scope` view.
    pub fn read_scope(&self, index: usize, max_pairs: usize, out: &mut Vec<(f32, f32)>) -> bool {
        out.clear();
        let Some(scope) = self.0.slots.get(index).and_then(|slot| slot.scope.as_ref()) else {
            return false;
        };
        scope.read_recent(max_pairs, out);
        true
    }
}

fn slot_reading(slot: &ProbeSlot) -> Option<ProbeReading> {
    let seq = slot.seq.load(Ordering::Acquire);
    (seq != 0).then(|| ProbeReading {
        last: f32::from_bits(slot.last.load(Ordering::Relaxed)),
        min: f32::from_bits(slot.min.load(Ordering::Relaxed)),
        max: f32::from_bits(slot.max.load(Ordering::Relaxed)),
        seq,
    })
}

/// The probe set `instance` exposes right now, or `None` when it has no
/// probes or is not live. Takes the registry lock once (and frees sets
/// retired by earlier rebuilds when no audio-thread reader is active).
pub fn probe_set(instance: ProbeInstance) -> Option<ProbeSetHandle> {
    instance_set(instance).map(ProbeSetHandle)
}

/// The probes `instance` exposes, in manifest order. Empty when it has none
/// or is not live.
pub fn probe_infos(instance: ProbeInstance) -> Vec<DGenProbe> {
    probe_set(instance)
        .map(|set| set.probes().cloned().collect())
        .unwrap_or_default()
}

/// The latest block summary of probe `id`#`occurrence`. `None` when the
/// instance or probe does not exist or no block has been captured yet. One
/// registry lookup per call: a poller reading every probe of an instance
/// uses [`probe_set`] instead.
pub fn probe_snapshot(instance: ProbeInstance, id: &str, occurrence: u32) -> Option<ProbeReading> {
    let set = instance_set(instance)?;
    slot_reading(set.slot(id, occurrence)?)
}

/// Fill `out` with the most recent `max_pairs` scope min/max pairs of probe
/// `id`#`occurrence`, oldest first. Returns `false` (and leaves `out` empty)
/// when the probe is missing or is not a `scope` view.
pub fn probe_scope_window(
    instance: ProbeInstance,
    id: &str,
    occurrence: u32,
    max_pairs: usize,
    out: &mut Vec<(f32, f32)>,
) -> bool {
    out.clear();
    let Some(set) = instance_set(instance) else {
        return false;
    };
    let Some(scope) = set
        .slot(id, occurrence)
        .and_then(|slot| slot.scope.as_ref())
    else {
        return false;
    };
    scope.read_recent(max_pairs, out);
    true
}

#[cfg(test)]
mod tests;
