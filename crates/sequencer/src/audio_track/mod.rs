//! Audio-track playback (docs/audio-tracks-spec.md §9.2): the region player
//! graph node and the lock-free seam the control thread publishes regions
//! through.
//!
//! Each audio track owns one region player node feeding its voice sums. The
//! node reads an immutable [`AudioTrackPlayback`] through its
//! [`AudioTrackSlot`]: the control thread builds a new snapshot on every
//! arrangement change and swaps the slot's pointer; the audio thread loads it
//! once per block. Retired snapshots — and the source PCM they hold — are
//! freed on the control thread once the audio thread has finished a block
//! that started after the swap ([`AudioTrackPlayers::reclaim`]), never on the
//! audio thread (eseq-sj01).
//!
//! Playback is arrangement-only: the node maps each rendered sample to a
//! song beat through `SongPositionShared::render_mapping`, so session play
//! and a stopped transport are silence.
//!
//! Region locks (eseq-a921) are applied by the audio CALLBACK, not the node:
//! the param ring has one producer, and nodes run on parallel workers. Once
//! per block [`apply_region_locks`] finds the region each locked track is in
//! and pushes its locked values (the track values when it leaves one).

use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicI64, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::audiograph::NodeVTable;
use crate::instruments::sample_core;
use crate::sequencer::{crossfade_gains, fade_shape, SongPositionShared, SongRenderMapping};

/// Shortest fade at every region edge and on transport start/stop, so no
/// edge ever clicks (about 1.5 ms at 44.1 kHz).
pub const DECLICK_FRAMES: f64 = 64.0;

/// Decoded source audio: interleaved stereo at the file's native rate.
pub struct SourcePcm {
    pub samples: Box<[f32]>,
    pub sample_rate: u32,
}

impl SourcePcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }
}

/// One region as the audio thread plays it, precomputed so the per-frame
/// work is a lookup, two multiplies and an interpolated read.
#[derive(Clone)]
pub struct PlaybackRegion {
    pub start_beat: f64,
    pub end_beat: f64,
    /// Source frame heard at `start_beat`.
    pub source_offset: f64,
    /// Source frames per host frame: `sample_core::playback_ratio` times the
    /// region's pitch speed.
    pub step: f64,
    /// `[start, end)` source frames the region loops over, if it loops.
    pub loop_window: Option<(f64, f64)>,
    pub gain: f32,
    /// Host frames of fade at each edge (0: none); every edge also gets a
    /// [`DECLICK_FRAMES`] ramp.
    pub fade_in: f64,
    pub fade_out: f64,
    /// Fade shapes as ramp exponents (`fade_curve_power`).
    pub fade_in_power: f32,
    pub fade_out_power: f32,
    /// The crossfade with the region before this one (which ends where this
    /// starts): host frames of the window before and after this region's
    /// start, both 0 for none. Over the window the previous region keeps
    /// playing past its end and this one pre-rolls before its start, their
    /// gains following `crossfade_gains`.
    pub xfade_pre: f64,
    pub xfade_post: f64,
    pub pcm: Arc<SourcePcm>,
    /// `(index into AudioTrackPlayback::lock_params, value)` this region
    /// locks.
    pub locks: Vec<(u16, f32)>,
}

/// A parameter some region of the track locks: where it is written and the
/// track's own value it reverts to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockParam {
    pub logical_id: u64,
    pub idx: u64,
    pub base: f32,
}

/// Everything one region player plays: its track's regions, sorted and
/// non-overlapping (the arrangement's lane invariant).
#[derive(Clone, Default)]
pub struct AudioTrackPlayback {
    pub regions: Vec<PlaybackRegion>,
    /// Every parameter a region locks, plus any a previous snapshot locked
    /// (so a removed lock still reverts).
    pub lock_params: Vec<LockParam>,
    /// Set by [`AudioTrackPlayers::publish`], never reused: how the lock pass
    /// tells snapshots apart (a freed snapshot's address can come back).
    pub generation: u64,
}

impl AudioTrackPlayback {
    /// The region sounding at `beat`, checking `hint` (the last region
    /// played) before searching.
    fn region_at(&self, beat: f64, hint: usize) -> Option<usize> {
        let contains = |index: usize| {
            self.regions
                .get(index)
                .is_some_and(|region| beat >= region.start_beat && beat < region.end_beat)
        };
        if contains(hint) {
            return Some(hint);
        }
        if contains(hint + 1) {
            return Some(hint + 1);
        }
        let index = self.regions.partition_point(|region| region.end_beat <= beat);
        contains(index).then_some(index)
    }

    /// The stereo frame heard at `beat`. `beats_per_sample` converts song
    /// time to host frames.
    fn frame_at(&self, beat: f64, beats_per_sample: f64, hint: &mut usize) -> (f32, f32) {
        let Some(index) = self.region_at(beat, *hint) else {
            return (0.0, 0.0);
        };
        *hint = index;
        let region = &self.regions[index];
        let since_start = (beat - region.start_beat) / beats_per_sample;
        let until_end = (region.end_beat - beat) / beats_per_sample;
        let (mut left, mut right) = (0.0, 0.0);
        let mut envelope = 1.0f32;
        // The head: a crossfade with the region before, else the fade-in.
        let crossfade_in = region.crossfade_frames() > 0.0 && index > 0;
        if crossfade_in && since_start < region.xfade_post {
            let (outgoing, incoming) =
                crossfade_gains((since_start + region.xfade_pre) / region.crossfade_frames());
            envelope *= incoming;
            let (l, r) = self.regions[index - 1].voice_at(beat, beats_per_sample);
            left += l * outgoing;
            right += r * outgoing;
        } else if !crossfade_in {
            envelope *= edge_gain(since_start, region.fade_in, region.fade_in_power);
        }
        // The tail: a crossfade into the region after, else the fade-out.
        match self.regions.get(index + 1).filter(|next| next.crossfade_frames() > 0.0) {
            Some(next) => {
                if until_end < next.xfade_pre {
                    let (outgoing, incoming) =
                        crossfade_gains((next.xfade_pre - until_end) / next.crossfade_frames());
                    envelope *= outgoing;
                    let (l, r) = next.voice_at(beat, beats_per_sample);
                    left += l * incoming;
                    right += r * incoming;
                }
            }
            None => envelope *= edge_gain(until_end, region.fade_out, region.fade_out_power),
        }
        let (l, r) = region.voice_at(beat, beats_per_sample);
        (left + l * envelope, right + r * envelope)
    }
}

/// The level `distance` host frames into an edge fade of `length` frames
/// shaped by `power`, always under a [`DECLICK_FRAMES`] ramp.
fn edge_gain(distance: f64, length: f64, power: f32) -> f32 {
    let declick = (distance / DECLICK_FRAMES).clamp(0.0, 1.0) as f32;
    // Past the fade (most frames) the shape is 1: no `powf` per sample.
    if length > distance {
        declick * fade_shape(distance / length, power)
    } else {
        declick
    }
}

impl PlaybackRegion {
    /// Host frames of its crossfade with the region before (0: none).
    fn crossfade_frames(&self) -> f64 {
        self.xfade_pre + self.xfade_post
    }

    /// What the region's source plays at `beat` at its clip gain, before
    /// any envelope. A crossfade reads it past either edge; there is
    /// silence where the source has no audio.
    fn voice_at(&self, beat: f64, beats_per_sample: f64) -> (f32, f32) {
        let since_start = (beat - self.start_beat) / beats_per_sample;
        let mut position = self.source_offset + since_start * self.step;
        if let Some((start, end)) = self.loop_window {
            if position >= end {
                position = start + (position - end).rem_euclid(end - start);
            }
        }
        let (left, right) = sample_core::read_stereo_at(&self.pcm.samples, position);
        (left * self.gain, right * self.gain)
    }
}

/// The per-track seam between the control thread and a region player node.
/// Slots are owned by [`AudioTrackPlayers`] and outlive every node that
/// points at them.
pub struct AudioTrackSlot {
    playback: AtomicPtr<AudioTrackPlayback>,
    /// What [`apply_region_locks`] last applied (callback thread only): the
    /// snapshot's generation and the region index in it, `-1` for none.
    applied_playback: AtomicU64,
    applied_region: AtomicI64,
    song: *const SongPositionShared,
    /// `SequencerState::audio_rendered_sample`: while the graph renders a
    /// block it still holds that block's first sample.
    rendered: *const AtomicU64,
}

// SAFETY: the raw pointers address `SequencerState` fields behind the App's
// `Arc`, which outlives the graph; the playback pointer is atomic.
unsafe impl Send for AudioTrackSlot {}
unsafe impl Sync for AudioTrackSlot {}

/// Node state: the slot plus the transport ramp that keeps start, stop and
/// seeks click-free.
#[repr(C)]
#[derive(Clone, Copy)]
struct RegionPlayerState {
    slot: *const AudioTrackSlot,
    host_rate: f32,
    /// 0..1 output ramp; rises while the song plays, falls after it stops.
    ramp: f32,
    /// The last song beat played, extrapolated while ramping out.
    last_beat: f64,
    last_beats_per_sample: f64,
    region_hint: u64,
}

const STATE_SIZE: usize = std::mem::size_of::<RegionPlayerState>();

unsafe extern "C" fn region_player_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial: *const c_void,
) {
    if state.is_null() || initial.is_null() {
        return;
    }
    let slot = std::ptr::read_unaligned(initial as *const *const AudioTrackSlot);
    std::ptr::write_unaligned(
        state as *mut RegionPlayerState,
        RegionPlayerState {
            slot,
            host_rate: sample_rate.max(1) as f32,
            ramp: 0.0,
            last_beat: 0.0,
            last_beats_per_sample: 0.0,
            region_hint: 0,
        },
    );
}

unsafe extern "C" fn region_player_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if nframes <= 0 || state.is_null() {
        return;
    }
    let frames = nframes as usize;
    let left = std::slice::from_raw_parts_mut(*out.add(0), frames);
    let right = std::slice::from_raw_parts_mut(*out.add(1), frames);
    let mut player = std::ptr::read_unaligned(state as *const RegionPlayerState);
    let Some(slot) = player.slot.as_ref() else {
        left.fill(0.0);
        right.fill(0.0);
        return;
    };
    let playback = slot.playback.load(Ordering::Acquire).as_ref();
    let block_start = (*slot.rendered).load(Ordering::Acquire);
    let mapping = (*slot.song).render_mapping(block_start);
    render_block(&mut player, playback, mapping, block_start, left, right);
    std::ptr::write_unaligned(state as *mut RegionPlayerState, player);
}

/// The node's block body, separate from the FFI shell so tests drive it.
fn render_block(
    player: &mut RegionPlayerState,
    playback: Option<&AudioTrackPlayback>,
    mapping: Option<SongRenderMapping>,
    block_start: u64,
    left: &mut [f32],
    right: &mut [f32],
) {
    let ramp_step = (1.0 / DECLICK_FRAMES) as f32;
    let Some(playback) = playback.filter(|playback| !playback.regions.is_empty()) else {
        player.ramp = 0.0;
        left.fill(0.0);
        right.fill(0.0);
        return;
    };
    let mut hint = player.region_hint as usize;
    for (offset, (left, right)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
        let beat = mapping.and_then(|mapping| {
            let beat = mapping.song_beat(block_start + offset as u64, offset)?;
            Some((beat, mapping.beats_per_sample))
        });
        let (beat, beats_per_sample) = match beat {
            Some((beat, beats_per_sample)) => {
                // A seek or loop wrap jumps the song position: ramp from
                // silence instead of splicing two unrelated waveforms.
                let expected = player.last_beat + player.last_beats_per_sample;
                if player.ramp > 0.0
                    && player.last_beats_per_sample > 0.0
                    && (beat - expected).abs() > 2.0 * beats_per_sample
                {
                    player.ramp = 0.0;
                }
                player.ramp = (player.ramp + ramp_step).min(1.0);
                (beat, beats_per_sample)
            }
            None if player.ramp > 0.0 && player.last_beats_per_sample > 0.0 => {
                // Stopped: keep reading forward while the ramp falls.
                player.ramp = (player.ramp - ramp_step).max(0.0);
                (player.last_beat + player.last_beats_per_sample, player.last_beats_per_sample)
            }
            None => {
                player.ramp = 0.0;
                *left = 0.0;
                *right = 0.0;
                continue;
            }
        };
        player.last_beat = beat;
        player.last_beats_per_sample = beats_per_sample;
        let (l, r) = playback.frame_at(beat, beats_per_sample, &mut hint);
        *left = l * player.ramp;
        *right = r * player.ramp;
    }
    player.region_hint = hint as u64;
}

pub fn region_player_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(region_player_process),
        init: Some(region_player_init),
        ..NodeVTable::default()
    }
}

/// Graph identity of one track's region player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPlayerNode {
    pub node_id: i32,
    /// Index into [`AudioTrackPlayers`]' slots.
    pub slot: usize,
}

/// Control-thread owner of every slot and of retired snapshots.
#[derive(Default)]
pub struct AudioTrackPlayers {
    slots: Vec<Box<AudioTrackSlot>>,
    /// Swapped-out snapshots with the rendered sample seen at the swap.
    retired: Vec<(*mut AudioTrackPlayback, u64)>,
    rendered: Option<*const AtomicU64>,
    /// The last [`AudioTrackPlayback::generation`] handed out.
    generation: u64,
}

// SAFETY: see `AudioTrackSlot`; retired pointers are owned boxes.
unsafe impl Send for AudioTrackPlayers {}

impl AudioTrackPlayers {
    /// Create a slot and its region player node, wired into the track's
    /// voice sums.
    ///
    /// # Safety
    /// `lg` must be the live graph whose state `song` and `rendered` belong
    /// to, and both must outlive the graph.
    pub unsafe fn create_player(
        &mut self,
        lg: *mut crate::audiograph::LiveGraph,
        name: &str,
        song: &SongPositionShared,
        rendered: &AtomicU64,
        registry: &AudioSlotRegistry,
        voice_sum_l: i32,
        voice_sum_r: i32,
    ) -> Result<AudioPlayerNode, String> {
        let slot = Box::new(AudioTrackSlot {
            playback: AtomicPtr::new(std::ptr::null_mut()),
            applied_playback: AtomicU64::new(0),
            applied_region: AtomicI64::new(-1),
            song: song as *const SongPositionShared,
            rendered: rendered as *const AtomicU64,
        });
        self.rendered = Some(rendered as *const AtomicU64);
        let slot_ptr: *const AudioTrackSlot = &*slot;
        let node_name = CString::new(format!("{name}_regions"))
            .map_err(|_| "audio track name contains NUL".to_string())?;
        let node_id = crate::audiograph::add_node(
            lg,
            region_player_vtable(),
            STATE_SIZE,
            node_name.as_ptr(),
            0,
            2,
            (&slot_ptr as *const *const AudioTrackSlot).cast(),
            std::mem::size_of::<*const AudioTrackSlot>(),
        );
        if node_id < 0 {
            return Err(format!("failed to add region player for '{name}'"));
        }
        crate::audiograph::graph_connect(lg, node_id, 0, voice_sum_l, 0);
        crate::audiograph::graph_connect(lg, node_id, 1, voice_sum_r, 0);
        registry.register(slot_ptr);
        self.slots.push(slot);
        Ok(AudioPlayerNode { node_id, slot: self.slots.len() - 1 })
    }

    /// Publish `playback` to `slot`, retiring what it replaces.
    pub fn publish(&mut self, slot: usize, mut playback: AudioTrackPlayback) {
        let Some(slot) = self.slots.get(slot) else {
            return;
        };
        self.generation += 1;
        playback.generation = self.generation;
        let fresh = Box::into_raw(Box::new(playback));
        let old = slot.playback.swap(fresh, Ordering::AcqRel);
        if !old.is_null() {
            let stamp = self.rendered_now();
            self.retired.push((old, stamp));
        }
        self.reclaim();
    }

    /// Detach a deleted track's slot. The node may still run until its
    /// deletion applies, so the snapshot is retired, not freed.
    pub fn clear(&mut self, slot: usize) {
        let Some(slot) = self.slots.get(slot) else {
            return;
        };
        let old = slot.playback.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !old.is_null() {
            let stamp = self.rendered_now();
            self.retired.push((old, stamp));
        }
    }

    fn rendered_now(&self) -> u64 {
        self.rendered
            .map(|rendered| unsafe { (*rendered).load(Ordering::SeqCst) })
            .unwrap_or(0)
    }

    /// Free retired snapshots the audio thread can no longer be reading:
    /// any block in flight at the swap started at or before the stamp, and
    /// the rendered counter only passes the stamp once that block is done.
    pub fn reclaim(&mut self) {
        let now = self.rendered_now();
        self.retired.retain(|&(snapshot, stamp)| {
            if now > stamp {
                drop(unsafe { Box::from_raw(snapshot) });
                false
            } else {
                true
            }
        });
    }

    #[cfg(test)]
    pub fn retired_len(&self) -> usize {
        self.retired.len()
    }

    /// The snapshot `slot` currently plays (tests).
    #[cfg(test)]
    pub fn published(&self, slot: usize) -> Option<&AudioTrackPlayback> {
        unsafe { self.slots.get(slot)?.playback.load(Ordering::Acquire).as_ref() }
    }
}

/// Every slot, for the audio callback's lock pass. Slots are never freed
/// (see `Drop for AudioTrackPlayers`), so a registered pointer stays valid.
pub struct AudioSlotRegistry {
    slots: [AtomicPtr<AudioTrackSlot>; MAX_AUDIO_SLOTS],
    len: AtomicUsize,
}

/// More than `MAX_TRACKS`: a deleted track's slot is not reused.
const MAX_AUDIO_SLOTS: usize = 256;

impl Default for AudioSlotRegistry {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicPtr::new(std::ptr::null_mut())),
            len: AtomicUsize::new(0),
        }
    }
}

impl AudioSlotRegistry {
    fn register(&self, slot: *const AudioTrackSlot) {
        let index = self.len.load(Ordering::Acquire);
        if index < MAX_AUDIO_SLOTS {
            self.slots[index].store(slot as *mut AudioTrackSlot, Ordering::Release);
            self.len.store(index + 1, Ordering::Release);
        }
    }
}

/// Apply region locks for the block starting at `block_start`: for each
/// track whose snapshot locks anything, push every lock parameter's value
/// (the region's lock, else the track value) when the region it is in or
/// its snapshot changed, and re-assert the region's locks every block
/// otherwise, so a track-value write (a knob, a Play/Stop resync) cannot
/// undo a lock while its region plays.
///
/// # Safety
/// Audio callback thread only (the param ring's producer); `lg` is the live
/// graph the slots' nodes belong to.
pub unsafe fn apply_region_locks(
    lg: *mut crate::audiograph::LiveGraph,
    registry: &AudioSlotRegistry,
    block_start: u64,
) {
    region_lock_pass(registry, block_start, |param, value| unsafe {
        crate::audiograph::params_push_wrapper(
            lg,
            crate::audiograph::ParamMsg { idx: param.idx, logical_id: param.logical_id, fvalue: value },
        );
    });
}

/// [`apply_region_locks`]' body, with the param write abstracted (tests).
fn region_lock_pass(registry: &AudioSlotRegistry, block_start: u64, mut push: impl FnMut(&LockParam, f32)) {
    for slot in &registry.slots[..registry.len.load(Ordering::Acquire).min(MAX_AUDIO_SLOTS)] {
        let Some(slot) = (unsafe { slot.load(Ordering::Acquire).as_ref() }) else {
            continue;
        };
        let pointer = slot.playback.load(Ordering::Acquire);
        let Some(playback) = (unsafe { pointer.as_ref() }) else {
            continue;
        };
        if playback.lock_params.is_empty() {
            continue;
        }
        let beat = unsafe { (*slot.song).render_mapping(block_start) }
            .and_then(|mapping| mapping.song_beat(block_start, 0));
        apply_slot_locks(slot, playback, beat, &mut push);
    }
}

/// One slot's step of [`region_lock_pass`] at song `beat` (`None`: the
/// transport is stopped or outside the song).
fn apply_slot_locks(
    slot: &AudioTrackSlot,
    playback: &AudioTrackPlayback,
    beat: Option<f64>,
    push: &mut impl FnMut(&LockParam, f32),
) {
    let generation = playback.generation;
    let previous = slot.applied_region.load(Ordering::Relaxed);
    let region = beat
        .and_then(|beat| playback.region_at(beat, previous.max(0) as usize))
        .map_or(-1, |index| index as i64);
    let locks = usize::try_from(region).ok().map(|index| playback.regions[index].locks.as_slice());
    if slot.applied_playback.load(Ordering::Relaxed) != generation || previous != region {
        for (index, param) in playback.lock_params.iter().enumerate() {
            let locked = locks.and_then(|locks| locks.iter().find(|(i, _)| *i as usize == index));
            push(param, locked.map_or(param.base, |&(_, value)| value));
        }
        slot.applied_playback.store(generation, Ordering::Relaxed);
        slot.applied_region.store(region, Ordering::Relaxed);
    } else if let Some(locks) = locks {
        for &(index, value) in locks {
            if let Some(param) = playback.lock_params.get(index as usize) {
                push(param, value);
            }
        }
    }
}

impl Drop for AudioTrackPlayers {
    fn drop(&mut self) {
        // The graph is torn down after the App and may still be rendering:
        // nodes hold slot pointers, and a retired snapshot may still be read
        // by a block that started before its swap. Release them with the
        // process rather than racing the audio thread here.
        self.retired.clear();
        for slot in self.slots.drain(..) {
            std::mem::forget(slot);
        }
    }
}

#[cfg(test)]
mod tests;
