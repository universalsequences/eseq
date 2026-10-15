//! Beats-mode time stretch for the sampler (Ableton "Beats", not Complex).
//!
//! Two clocks run side by side, both in SOURCE frames:
//!
//! - The *timeline* is musical position. It advances by the warp ratio every
//!   host frame and is integrated, never recomputed from elapsed time, so the
//!   ratio (sample BPM) can be modulated freely without the read position
//!   teleporting.
//! - The *voice* reads audio. Each grid segment plays from its own start at
//!   the voice's playback rate (speed/transpose), so attacks keep their pitch
//!   and shape whatever the tempo.
//!
//! When the timeline crosses the next boundary, the current voice hands off
//! to a fading voice and a new one starts on the boundary. When the voice
//! runs out of segment before the timeline gets there (project slower than
//! the sample), the fill mode decides what plays: fade out, loop the
//! segment's tail forward, or bounce it back and forth. Every one of those
//! transitions is a complementary raised-cosine crossfade, every outgoing
//! voice keeps the gain it had, and no voice ever reads past its segment's
//! end, so the next transient can't leak in early.

/// Crossfade into a new segment. Short so attacks stay sharp; complementary
/// so identical audio on both sides (ratio 1) passes through unchanged.
const BOUNDARY_XFADE_SECONDS: f32 = 0.0025;
/// Crossfade for tail fills (loop / bounce / fade-out), where both sides are
/// sustained material and a longer blend hides the seam.
const FILL_XFADE_SECONDS: f32 = 0.010;
/// Ratio smoothing: fast enough for audio-rate-ish modulation, slow enough
/// that per-block steps don't zipper.
pub const RATIO_SMOOTH_SECONDS: f32 = 0.005;
/// Most boundaries crossed in one host frame before the voice just lands on
/// the latest one (absurd ratios on tiny grids).
const MAX_BOUNDARY_SKIPS: usize = 64;

/// Overlapping hand-offs (a boundary landing inside a tail crossfade, fast
/// ping-pong under modulation) each need their own fading voice; evicting
/// one that is still audible would be the very click this module avoids.
pub const FADE_VOICES: usize = 4;

pub const FILL_OFF: i32 = 0;
pub const FILL_LOOP: i32 = 1;
pub const FILL_PINGPONG: i32 = 2;

/// A voice that was handed off and is fading out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FadeVoice {
    pub pos: f32,
    pub dir: f32,
    /// Gain the voice had at hand-off; the fade shape scales it to zero.
    pub gain: f32,
    pub remaining: f32,
    pub total: f32,
}

impl FadeVoice {
    fn active(&self) -> bool {
        self.remaining > 0.0 && self.total > 0.0
    }

    /// Raised-cosine fade-out, complementary to `fade_in_shape`.
    fn level(&self) -> f32 {
        if !self.active() {
            return 0.0;
        }
        let t = 1.0 - (self.remaining / self.total).clamp(0.0, 1.0);
        self.gain * 0.5 * (1.0 + (std::f32::consts::PI * t).cos())
    }
}

/// One-pole smoothing coefficient for a time constant in seconds.
pub fn one_pole_coeff(sample_rate: f32, seconds: f32) -> f32 {
    1.0 - (-1.0 / (sample_rate * seconds).max(1.0)).exp()
}

fn fade_in_shape(remaining: f32, total: f32) -> f32 {
    if remaining <= 0.0 || total <= 0.0 {
        return 1.0;
    }
    let t = 1.0 - (remaining / total).clamp(0.0, 1.0);
    0.5 * (1.0 - (std::f32::consts::PI * t).cos())
}

/// Per-block inputs.
pub struct BeatsBlock {
    pub start: f32,
    /// Exclusive region end (source frames).
    pub end: f32,
    pub host_sample_rate: f32,
    /// Source frames per host frame at rate 1.
    pub src_per_host: f32,
    /// Tempo ratio (project / sample BPM) the timeline glides toward.
    pub target_ratio: f32,
    /// `one_pole_coeff(host_sample_rate, RATIO_SMOOTH_SECONDS)`.
    pub ratio_coeff: f32,
    pub fill: i32,
    /// 0 = segments sustain, 1 = each segment decays hard (choppy).
    pub decay: f32,
    /// Region loop: the timeline wraps instead of ending.
    pub region_loop: bool,
}

/// Per-frame inputs.
pub struct BeatsFrame {
    /// Voice read speed in source frames per host frame (speed, transpose
    /// and the scrub hand already folded in).
    pub read_step: f32,
    /// Scrub hand multiplier: holding the record slows musical time too.
    pub timeline_mul: f32,
    /// Scrub offset in source frames, applied to every voice alike so a
    /// hand-off never jumps by it.
    pub scrub_frames: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatsWarp {
    pub timeline: f64,
    /// Current voice read position (source frames, before scrub).
    pub pos: f32,
    /// +1 forward, -1 bouncing back (ping-pong fill), 0 silent (fill off).
    pub dir: f32,
    pub fade_in_remaining: f32,
    pub fade_in_total: f32,
    pub seg_start: f32,
    pub next_boundary: f32,
    pub ratio: f32,
    pub fades: [FadeVoice; FADE_VOICES],
    /// Region ended (no loop): the last voice is fading out.
    pub ending: bool,
}

pub struct BeatsOut {
    pub left: f32,
    pub right: f32,
    /// Everything has faded after the region end; the note is over.
    pub finished: bool,
    /// Where the current voice read this frame (for the retrigger tail and
    /// the UI playhead).
    pub read_head: f32,
}

impl BeatsWarp {
    /// Fresh note (or warp switched on mid-note): start at `pos` with no
    /// crossfade from anything.
    pub fn reset(&mut self, pos: f32, ratio: f32, next_boundary: impl Fn(f32) -> f32) {
        *self = Self {
            timeline: pos as f64,
            pos,
            dir: 1.0,
            seg_start: pos,
            next_boundary: next_boundary(pos),
            ratio,
            ..Self::default()
        };
    }

    /// State that can't belong to a running Beats note (warp toggled on
    /// mid-note, region moved under it).
    pub fn is_stale(&self, block: &BeatsBlock) -> bool {
        !self.timeline.is_finite()
            || self.timeline < block.start as f64 - 1.0
            || self.next_boundary <= self.seg_start
            || self.ratio <= 0.0
    }

    /// Re-derive the next boundary from the segment start with this block's
    /// grid (sample BPM / preserve may have moved). Never moves the voice.
    pub fn refresh_grid(&mut self, next_boundary: impl Fn(f32) -> f32) {
        self.next_boundary = next_boundary(self.seg_start);
    }

    fn segment_gain(&self, decay: f32) -> f32 {
        if decay <= 0.001 {
            return 1.0;
        }
        let len = (self.next_boundary - self.seg_start).max(1.0) as f64;
        let phase = ((self.timeline - self.seg_start as f64) / len).max(0.0) as f32;
        // Time constant in segment lengths: 4 (barely audible) .. 0.15 (gated).
        let tau = 4.0 + (0.15 - 4.0) * decay.clamp(0.0, 1.0);
        (-phase / tau).exp()
    }

    fn current_gain(&self, decay: f32) -> f32 {
        if self.dir == 0.0 {
            return 0.0;
        }
        self.segment_gain(decay) * fade_in_shape(self.fade_in_remaining, self.fade_in_total)
    }

    /// Hand the current voice to a fade slot and let a new one fade in over
    /// `frames` host frames. Gains stay continuous: the outgoing voice keeps
    /// exactly the gain it had.
    fn hand_off(&mut self, frames: f32, decay: f32) {
        let frames = frames.max(1.0);
        let gain = self.current_gain(decay);
        if gain > 0.0 {
            // A free slot, else the quietest fade (all busy is rare: it
            // takes FADE_VOICES hand-offs inside one crossfade).
            let slot = self
                .fades
                .iter()
                .position(|fade| !fade.active())
                .unwrap_or_else(|| {
                    (0..FADE_VOICES)
                        .min_by(|&a, &b| self.fades[a].level().total_cmp(&self.fades[b].level()))
                        .unwrap_or(0)
                });
            self.fades[slot] = FadeVoice {
                pos: self.pos,
                dir: self.dir,
                gain,
                remaining: frames,
                total: frames,
            };
        }
        self.fade_in_remaining = frames;
        self.fade_in_total = frames;
    }

    /// Start the segment holding `timeline`, at or after `from`.
    fn enter_segment(
        &mut self,
        from: f32,
        block: &BeatsBlock,
        next_boundary: &impl Fn(f32) -> f32,
    ) {
        let mut seg = from;
        let mut next = next_boundary(seg);
        let mut skips = 0;
        while (next as f64) <= self.timeline && next < block.end && skips < MAX_BOUNDARY_SKIPS {
            seg = next;
            next = next_boundary(seg);
            skips += 1;
        }
        self.hand_off(block.host_sample_rate * BOUNDARY_XFADE_SECONDS, block.decay);
        self.seg_start = seg;
        self.next_boundary = next;
        self.pos = seg;
        self.dir = 1.0;
    }

    /// Fill when the voice is about to run out of segment before the
    /// timeline reaches the boundary.
    fn fill(&mut self, block: &BeatsBlock, frame: &BeatsFrame) {
        if self.dir == 0.0 {
            return;
        }
        let step = frame.read_step.abs().max(1e-6);
        let content_end = self.next_boundary.min(block.end);
        let seg_len = (content_end - self.seg_start).max(0.0);
        // Crossfade length in source frames; a quarter segment at most so a
        // loop window always holds two crossfades.
        let xf_src = (FILL_XFADE_SECONDS * block.host_sample_rate * step).min(seg_len * 0.25);
        let loop_start = self.seg_start + 0.5 * seg_len;

        if self.dir > 0.0 {
            if self.pos < content_end - xf_src {
                return;
            }
            // If the boundary arrives before the content runs out, it cuts
            // the segment itself; filling would only smear it (ratio >= 1).
            let tl_rate = (self.ratio * block.src_per_host * frame.timeline_mul).max(1e-6) as f64;
            let frames_to_boundary = (content_end as f64 - self.timeline).max(0.0) / tl_rate;
            let frames_to_content_end = ((content_end - self.pos).max(0.0) / step) as f64;
            if frames_to_boundary <= frames_to_content_end {
                return;
            }
        } else if self.pos > loop_start + xf_src {
            return;
        }
        self.hand_off((xf_src / step).max(1.0), block.decay);
        if self.dir < 0.0 {
            // Bouncing back (ping-pong, or the mode changed mid-bounce):
            // turn forward again at the tail window's start.
            self.dir = 1.0;
            return;
        }
        match block.fill {
            FILL_LOOP => self.pos = loop_start,
            FILL_PINGPONG => self.dir = -1.0,
            _ => self.dir = 0.0,
        }
    }

    /// Render one host frame. `read(pos)` returns the interpolated source
    /// frame at `pos` (already clamped to the region by the caller's reader).
    pub fn tick(
        &mut self,
        block: &BeatsBlock,
        frame: &BeatsFrame,
        next_boundary: &impl Fn(f32) -> f32,
        read: &impl Fn(f32) -> (f32, f32),
    ) -> BeatsOut {
        self.ratio += (block.target_ratio - self.ratio) * block.ratio_coeff;

        // ── Events (before reading, so a hand-off is heard this frame) ──
        if !self.ending {
            if self.timeline >= block.end as f64 {
                if block.region_loop && block.end > block.start {
                    let len = (block.end - block.start) as f64;
                    self.timeline = block.start as f64 + (self.timeline - block.end as f64) % len;
                    self.enter_segment(block.start, block, next_boundary);
                } else {
                    self.hand_off(block.host_sample_rate * FILL_XFADE_SECONDS, block.decay);
                    self.dir = 0.0;
                    self.ending = true;
                }
            } else if self.timeline >= self.next_boundary as f64 && self.next_boundary < block.end {
                let from = self.next_boundary;
                self.enter_segment(from, block, next_boundary);
            } else {
                self.fill(block, frame);
            }
        }

        // ── Mix ──
        let read_at = |pos: f32| {
            (pos + frame.scrub_frames).clamp(block.start, (block.end - 1.0).max(block.start))
        };
        let mut left = 0.0;
        let mut right = 0.0;
        let read_head = read_at(self.pos);
        let gain = self.current_gain(block.decay);
        if gain > 0.0 {
            let (l, r) = read(read_head);
            left += l * gain;
            right += r * gain;
        }
        for fade in &self.fades {
            let level = fade.level();
            if level > 0.0 {
                let (l, r) = read(read_at(fade.pos));
                left += l * level;
                right += r * level;
            }
        }

        // ── Advance ──
        if self.dir != 0.0 {
            self.pos += frame.read_step * self.dir;
        }
        if self.fade_in_remaining > 0.0 {
            self.fade_in_remaining -= 1.0;
        }
        for fade in &mut self.fades {
            if fade.active() {
                fade.pos += frame.read_step * fade.dir;
                fade.remaining -= 1.0;
            }
        }
        if !self.ending {
            self.timeline += (self.ratio * block.src_per_host * frame.timeline_mul) as f64;
        }

        let finished = self.ending && !self.fades.iter().any(FadeVoice::active);
        BeatsOut {
            left,
            right,
            finished,
            read_head,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(div: f32) -> impl Fn(f32) -> f32 {
        move |after: f32| ((after / div).floor() + 1.0) * div
    }

    fn block(fill: i32, ratio: f32) -> BeatsBlock {
        BeatsBlock {
            start: 0.0,
            end: 100_000.0,
            host_sample_rate: 1_000.0,
            src_per_host: 1.0,
            target_ratio: ratio,
            ratio_coeff: one_pole_coeff(1_000.0, RATIO_SMOOTH_SECONDS),
            fill,
            decay: 0.0,
            region_loop: false,
        }
    }

    fn frame() -> BeatsFrame {
        BeatsFrame {
            read_step: 1.0,
            timeline_mul: 1.0,
            scrub_frames: 0.0,
        }
    }

    #[test]
    fn unity_ratio_reads_straight_through_boundaries() {
        let next = grid(250.0);
        let b = block(FILL_LOOP, 1.0);
        let mut warp = BeatsWarp::default();
        warp.reset(0.0, 1.0, &next);
        let read = |pos: f32| (pos, pos);
        for i in 0..2_000 {
            let out = warp.tick(&b, &frame(), &next, &read);
            assert!(
                (out.left - i as f32).abs() < 1e-3,
                "frame {i}: {}",
                out.left
            );
        }
    }

    #[test]
    fn outgoing_voice_never_reads_past_its_segment() {
        // Slow ratio + loop fill: nothing at or past a boundary may sound
        // before the timeline reaches that boundary.
        let next = grid(250.0);
        let b = block(FILL_LOOP, 0.4);
        let mut warp = BeatsWarp::default();
        warp.reset(0.0, 0.4, &next);
        // Marker: 1.0 inside segment 0, 0.0 elsewhere.
        let read = |pos: f32| if pos < 250.0 { (1.0, 1.0) } else { (0.0, 0.0) };
        for i in 0..620 {
            let out = warp.tick(&b, &frame(), &next, &read);
            // Timeline reaches 250 at host frame 625.
            assert!(
                (out.left - 1.0).abs() < 1e-3,
                "frame {i} leaked: {}",
                out.left
            );
        }
    }

    #[test]
    fn ratio_change_never_moves_the_voice() {
        let next = grid(250.0);
        let b = block(FILL_LOOP, 1.0);
        let mut warp = BeatsWarp::default();
        warp.reset(0.0, 1.0, &next);
        let read = |_: f32| (0.0, 0.0);
        for _ in 0..100 {
            warp.tick(&b, &frame(), &next, &read);
        }
        let before = warp.pos;
        warp.tick(&block(FILL_LOOP, 3.0), &frame(), &next, &read);
        assert!((warp.pos - (before + 1.0)).abs() < 1e-4);
    }
}
