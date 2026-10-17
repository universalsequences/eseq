//! Mod input → audio callback mailbox for mod input roles
//! (docs/mod-input-roles-spec.md §6).
//!
//! A track's four `mod_in_clip` nodes copy their clamped block into a lane
//! here while the callback has armed the track. After the graph renders, the
//! callback reads the lanes back on the same thread to detect triggers and
//! sample value roles. The graph has finished every node by the time
//! `process_next_block` returns, so relaxed atomics are enough: they keep the
//! cross-thread stores sound, and the engine's block join orders them.
//!
//! One live graph per process owns track mod inputs (offline export runs in
//! its own worker process), so the lanes are process-wide statics, like the
//! sampler activity masks in `voice_modulator`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::sequencer::{EXT_MOD_INPUT_COUNT, MAX_TRACKS};

/// Longest block a lane holds. The live engine renders 512-frame blocks;
/// frames past this read as silence.
pub const LANE_FRAMES: usize = 1024;
const LANE_COUNT: usize = MAX_TRACKS * EXT_MOD_INPUT_COUNT;
/// `WRITTEN` value for a lane its clip node has not published this block.
const NOT_WRITTEN: u32 = u32::MAX;

const ZERO: AtomicU32 = AtomicU32::new(0);
const EMPTY_LANE: [AtomicU32; LANE_FRAMES] = [ZERO; LANE_FRAMES];
const UNWRITTEN: AtomicU32 = AtomicU32::new(NOT_WRITTEN);
const DISARMED: AtomicBool = AtomicBool::new(false);

static ARMED: [AtomicBool; MAX_TRACKS] = [DISARMED; MAX_TRACKS];
static SAMPLES: [[AtomicU32; LANE_FRAMES]; LANE_COUNT] = [EMPTY_LANE; LANE_COUNT];
/// Frames published this block: `NOT_WRITTEN`, 0 for a silent block, or the
/// block length.
static WRITTEN: [AtomicU32; LANE_COUNT] = [UNWRITTEN; LANE_COUNT];

/// The lane a track's mod input publishes to, as stored (plus one) in the
/// clip node's `MOD_IN_CLIP_STATE_PUBLISH` cell.
pub fn lane_index(track: usize, input: usize) -> Option<usize> {
    (track < MAX_TRACKS && input < EXT_MOD_INPUT_COUNT).then_some(track * EXT_MOD_INPUT_COUNT + input)
}

/// Arm or disarm a track for this block and forget its last block's samples.
/// Called by the audio callback before the graph renders.
pub fn begin_block(track: usize, armed: bool) {
    if track >= MAX_TRACKS {
        return;
    }
    ARMED[track].store(armed, Ordering::Relaxed);
    for input in 0..EXT_MOD_INPUT_COUNT {
        WRITTEN[track * EXT_MOD_INPUT_COUNT + input].store(NOT_WRITTEN, Ordering::Relaxed);
    }
}

/// Whether a clip node publishing to `lane` should copy its block this time.
pub fn lane_armed(lane: usize) -> bool {
    lane < LANE_COUNT && ARMED[lane / EXT_MOD_INPUT_COUNT].load(Ordering::Relaxed)
}

/// Publish one block of clamped mod input. `None` = the block was silent.
pub fn publish(lane: usize, samples: Option<&[f32]>) {
    if lane >= LANE_COUNT {
        return;
    }
    let frames = match samples {
        Some(samples) => {
            let frames = samples.len().min(LANE_FRAMES);
            for (cell, value) in SAMPLES[lane].iter().zip(&samples[..frames]) {
                cell.store(value.to_bits(), Ordering::Relaxed);
            }
            frames as u32
        }
        None => 0,
    };
    WRITTEN[lane].store(frames, Ordering::Relaxed);
}

/// One published block, read back by the callback after the graph renders.
#[derive(Clone, Copy)]
pub struct LaneBlock {
    lane: usize,
    frames: usize,
}

impl LaneBlock {
    /// The lane's block, or `None` when its clip node did not publish.
    pub fn read(track: usize, input: usize) -> Option<Self> {
        let lane = lane_index(track, input)?;
        let written = WRITTEN[lane].load(Ordering::Relaxed);
        (written != NOT_WRITTEN).then_some(Self { lane, frames: written as usize })
    }

    /// The input's value at `frame`; silence and frames past the published
    /// block read as 0.
    pub fn value(&self, frame: usize) -> f32 {
        if frame >= self.frames {
            return 0.0;
        }
        f32::from_bits(SAMPLES[self.lane][frame].load(Ordering::Relaxed))
    }
}
