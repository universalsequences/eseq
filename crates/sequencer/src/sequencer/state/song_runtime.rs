//! Song-mode runtime types (docs/song-mode-spec.md sections 9 and 10).
//!
//! `RuntimeSong`/`RuntimeSongRow` are the immutable preflight product: every
//! row of the committed song resolved against the live project, carrying a
//! prebuilt `Arc<SequencerSnapshot>` so a row transition on the scheduler
//! thread is nothing but an `Arc` pointer switch — no scene/project mutexes,
//! no pattern cloning, no allocation, no asset loading (spec 9).
//!
//! `SongPlaybackRuntime` is the scheduler-owned playback cursor: it maps row
//! start beats to exact sample positions (without snapping to any grid, spec
//! 8.2), clamps each lookahead chunk to the next row boundary so a boundary
//! inside a block splits scheduling (spec 10.2/14.3), and drives end/loop
//! behaviour (spec 7.3.5).
//!
//! `SongPlaybackMailbox` is the cross-thread seam: bounded command channel
//! (control -> scheduler), bounded notice channel carrying
//! `AudibleSongRowApplied` records (scheduler -> control; Slice C capture
//! consumes these), and lock-free position atomics for a render-rate
//! `song-position-beats` read (spec 10.2).

use super::*;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

const SONG_COMMAND_CAPACITY: usize = 8;
const SONG_NOTICE_CAPACITY: usize = 256;

/// One preflighted song row: the persisted row state plus its resolved
/// per-track patterns and the prebuilt scheduler snapshot (spec 10.1).
#[derive(Clone, Debug)]
pub struct RuntimeSongRow {
    pub id: SongRowId,
    pub start_beat: f64,
    /// `None` for an unscened row (empty-arrangement spec 4.2): the row
    /// carries no scene identity and switches nothing when it starts.
    pub scene: Option<usize>,
    /// The row's complete override set as live pool ids (`None` =
    /// explicit-empty: the track is silenced for the row).
    pub overrides: Vec<(usize, Option<PatternId>)>,
    /// Effective pattern per track: override, else scene cell, else `None`
    /// (the track is scene-silenced for this row). For a take lane this is
    /// the CURRENT CHUNK's pool pattern (the row content), which changes at
    /// chunk boundaries — identity comparisons must use `resolved_sources`.
    pub resolved_pattern_ids: Vec<Option<PatternId>>,
    /// Source identity per track (takes spec 7.3): the `TakeId` for take
    /// lanes — never the current chunk's pattern id, so crossing a chunk
    /// boundary is not a source change (no accumulator reset, no retrig).
    pub resolved_sources: Vec<LaneSource>,
    /// Per-track clip start offset in fractional pattern steps (takes spec
    /// 7.1): the override's stored `offset_steps`, `0.0` for scene-resolved
    /// lanes. Consumed with the row's `start_beat` as the per-lane phase
    /// anchor by the scheduler clock.
    pub lane_offsets: Vec<f64>,
    /// Complete scheduler snapshot for this row, materialized outside the
    /// audio callback. Swapping to it at a boundary is allocation-free.
    pub scheduler_snapshot: Arc<SequencerSnapshot>,
}

/// The immutable preflight product handed to the scheduler at song start.
/// Start-position independent: valid for playback beginning at any beat.
#[derive(Clone, Debug)]
pub struct RuntimeSong {
    pub rows: Vec<RuntimeSongRow>,
    pub end_beat: f64,
    pub loop_enabled: bool,
    /// The loop brace `[start, end)` playback wraps inside while
    /// `loop_enabled` (`ProjectArrangement::loop_window`).
    pub loop_window: (f64, f64),
}

/// The song beat heard at unwrapped `beat`, anchored at `anchor` (the beat
/// last published), under loop brace `[start, end)`. Ableton's rule: a
/// playhead that reaches the loop end wraps to the start; one anchored at or
/// past the end plays straight on. When the anchor is a wrap (`wrapped`), a
/// beat short of `start` is the render lagging that wrap, so it maps back to
/// the brace's tail; otherwise it is genuinely before the brace.
pub fn loop_song_beat(beat: f64, anchor: f64, wrapped: bool, (start, end): (f64, f64)) -> f64 {
    let len = end - start;
    if !(len > 0.0) || anchor >= end {
        return beat;
    }
    if beat >= end || (wrapped && beat < start) {
        start + (beat - start).rem_euclid(len)
    } else {
        beat
    }
}

impl RuntimeSong {
    /// Index of the row governing `beat` (greatest `start_beat <= beat`),
    /// mirroring `state_at_beat` (spec 5.5). Assumes `beat` is already
    /// loop-normalized and within `[0, end_beat)`.
    pub fn row_index_at_beat(&self, beat: f64) -> Option<usize> {
        self.rows
            .iter()
            .rposition(|row| row.start_beat <= beat)
    }

    /// Validate a requested transport start on the song timeline. Any
    /// beat from 0 on is legal: a start at or past the end is open-ended
    /// jam room (unified-transport spec 4.2) where the transport
    /// auto-latches the selected scene, and a start past the loop brace
    /// plays straight on, as in Ableton.
    pub fn normalize_start_beat(&self, start_beat: f64) -> Result<f64, String> {
        if !start_beat.is_finite() || start_beat < 0.0 {
            return Err(format!("invalid song start beat {start_beat}"));
        }
        Ok(start_beat)
    }

    /// The loop brace, while looping is on and the brace is non-empty.
    pub fn active_loop(&self) -> Option<(f64, f64)> {
        let (start, end) = self.loop_window;
        (self.loop_enabled && end > start).then_some((start, end))
    }
}

/// Scheduler-authoritative record of one audible row transition: the row
/// identity plus the exact beat/sample at which it became effective
/// (spec 10.2). Slice C capture consumes these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudibleSongRowApplied {
    pub row_id: SongRowId,
    pub row_ordinal: usize,
    /// Song-timeline beat at which the row became audible (unquantized rows
    /// keep their exact stored beat; never snapped).
    pub effective_beat: f64,
    /// Sample position on the scheduler/audio timeline.
    pub effective_sample: u64,
    /// True when this application came from a loop wrap back to the loop
    /// start.
    pub wrapped: bool,
}

/// Scheduler -> control notifications for song playback.
#[derive(Clone, Debug, PartialEq)]
pub enum SongPlaybackNotice {
    RowApplied(AudibleSongRowApplied),
    /// The song reached `end_beat` with looping disabled. The scheduler has
    /// stopped scheduling; the control thread must stop the transport.
    Ended { end_beat: f64, end_sample: u64 },
    /// A `Start` command could not be applied on the scheduler thread.
    StartFailed { error: String },
}

/// Control -> scheduler song playback commands.
#[derive(Clone)]
pub enum SongPlaybackCommand {
    Start {
        song: Arc<RuntimeSong>,
        start_beat: f64,
        /// Arrangement capture (docs/song-mode-spec.md 7.4): the song end is
        /// NOT a stopping point while recording. The runtime holds the last
        /// row past `end_beat` instead of ending, so a take that grooves
        /// past the old song length is not cut off and committed there; the
        /// stop-commit extends `end_beat` to the Stop beat. Looping songs
        /// are unaffected — they already never end.
        open_ended: bool,
    },
    /// Re-preflighted rows for the song already playing (takes spec 16.7):
    /// swapped in place, keeping the cursor. Ignored if the row layout moved.
    Refresh {
        song: Arc<RuntimeSong>,
    },
    /// Structurally rebuilt arrangement rows. The scheduler remaps by its
    /// clock-derived song beat and either installs immediately when the
    /// sounding source/offset identity is unchanged or defers to the next
    /// row boundary.
    Rebuild {
        song: Arc<RuntimeSong>,
    },
    Stop,
}

/// Lock-free position state the scheduler publishes so the UI can derive a
/// smooth fractional `song-position-beats` every frame without locking
/// scheduler state (spec 10.2).
pub struct SongPositionShared {
    active: AtomicBool,
    ended: AtomicBool,
    loop_enabled: AtomicBool,
    /// Capture is running past the song end, so the position must not clamp
    /// there (it would pin every lane's playhead at the old end marker).
    open_ended: AtomicBool,
    anchor_sample: AtomicU64,
    /// First sample of the current playback pass: the scheduler publishes
    /// the start anchor a lookahead ahead of rendering, and samples rendered
    /// before it belong to the stopped transport, not to the song.
    pass_start_sample: AtomicU64,
    anchor_beat_bits: AtomicU64,
    samples_per_quarter_bits: AtomicU64,
    end_beat_bits: AtomicU64,
    loop_start_bits: AtomicU64,
    loop_end_bits: AtomicU64,
    /// The anchor was published at a loop wrap (see [`loop_song_beat`]).
    anchor_wrapped: AtomicBool,
    current_row: AtomicU64,
    current_row_id: AtomicU64,
}

impl Default for SongPositionShared {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            loop_enabled: AtomicBool::new(false),
            open_ended: AtomicBool::new(false),
            anchor_sample: AtomicU64::new(0),
            pass_start_sample: AtomicU64::new(0),
            anchor_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
            samples_per_quarter_bits: AtomicU64::new(0.0_f64.to_bits()),
            end_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
            loop_start_bits: AtomicU64::new(0.0_f64.to_bits()),
            loop_end_bits: AtomicU64::new(0.0_f64.to_bits()),
            anchor_wrapped: AtomicBool::new(false),
            current_row: AtomicU64::new(0),
            current_row_id: AtomicU64::new(0),
        }
    }
}

impl SongPositionShared {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn current_row_ordinal(&self) -> usize {
        self.current_row.load(Ordering::Relaxed) as usize
    }

    pub fn current_row_id(&self) -> SongRowId {
        SongRowId(self.current_row_id.load(Ordering::Relaxed))
    }

    /// The published loop brace, while looping is on.
    fn active_loop(&self) -> Option<(f64, f64)> {
        self.loop_enabled.load(Ordering::Relaxed).then(|| {
            (
                f64::from_bits(self.loop_start_bits.load(Ordering::Relaxed)),
                f64::from_bits(self.loop_end_bits.load(Ordering::Relaxed)),
            )
        })
    }

    /// Derive the song position in beats from a rendered sample position.
    /// Returns `None` while song playback is inactive.
    pub fn position_beats(&self, rendered_sample: u64) -> Option<f64> {
        if !self.is_active() {
            return None;
        }
        let end_beat = f64::from_bits(self.end_beat_bits.load(Ordering::Relaxed));
        if self.ended.load(Ordering::Relaxed) {
            return Some(end_beat);
        }
        let anchor_sample = self.anchor_sample.load(Ordering::Relaxed);
        let anchor_beat = f64::from_bits(self.anchor_beat_bits.load(Ordering::Relaxed));
        let samples_per_quarter =
            f64::from_bits(self.samples_per_quarter_bits.load(Ordering::Relaxed));
        if !(samples_per_quarter > 0.0) {
            return Some(anchor_beat);
        }
        // Signed: near a loop wrap the scheduler publishes the new anchor a
        // lookahead window ahead of rendering, so rendered may briefly trail.
        let delta = rendered_sample as f64 - anchor_sample as f64;
        let beats = anchor_beat + delta / samples_per_quarter;
        if let Some(window) = self.active_loop() {
            let wrapped = self.anchor_wrapped.load(Ordering::Relaxed);
            Some(loop_song_beat(beats, anchor_beat, wrapped, window))
        } else if self.open_ended.load(Ordering::Relaxed) {
            Some(beats.max(0.0))
        } else {
            Some(beats.clamp(0.0, end_beat))
        }
    }

    /// The song-timeline mapping audio rendering needs for a block starting
    /// at `block_start` (docs/audio-tracks-spec.md §9.2). `None` while song
    /// playback is inactive. The scheduler marks the end a lookahead before
    /// rendering reaches it, so an ended song still maps: frames past the end
    /// fall out in [`SongRenderMapping::song_beat`]. Unlike [`Self::position_beats`]
    /// the beat is neither wrapped nor clamped: a renderer advances it per
    /// frame and applies [`SongRenderMapping::song_beat`] itself, so a loop
    /// wrap inside a block lands on the exact frame.
    pub fn render_mapping(&self, block_start: u64) -> Option<SongRenderMapping> {
        if !self.is_active() {
            return None;
        }
        let samples_per_quarter =
            f64::from_bits(self.samples_per_quarter_bits.load(Ordering::Relaxed));
        if !(samples_per_quarter > 0.0) {
            return None;
        }
        let anchor_sample = self.anchor_sample.load(Ordering::Relaxed);
        let anchor_beat = f64::from_bits(self.anchor_beat_bits.load(Ordering::Relaxed));
        let delta = block_start as f64 - anchor_sample as f64;
        let end_beat = f64::from_bits(self.end_beat_bits.load(Ordering::Relaxed));
        Some(SongRenderMapping {
            beat: anchor_beat + delta / samples_per_quarter,
            beats_per_sample: 1.0 / samples_per_quarter,
            pass_start_sample: self.pass_start_sample.load(Ordering::Relaxed),
            end_beat,
            anchor_beat,
            anchor_wrapped: self.anchor_wrapped.load(Ordering::Relaxed),
            loop_window: self.active_loop(),
            open_ended: self.open_ended.load(Ordering::Relaxed),
        })
    }

    /// The sample the current (or last) pass started on.
    pub fn pass_start_sample(&self) -> u64 {
        self.pass_start_sample.load(Ordering::Relaxed)
    }

    fn mark_pass_start(&self, sample: u64) {
        self.pass_start_sample.store(sample, Ordering::Relaxed);
    }

    fn publish(
        &self,
        anchor_sample: u64,
        anchor_beat: f64,
        samples_per_quarter: f64,
        end_beat: f64,
        loop_window: Option<(f64, f64)>,
        anchor_wrapped: bool,
        open_ended: bool,
        row_ordinal: usize,
        row_id: SongRowId,
    ) {
        self.anchor_wrapped.store(anchor_wrapped, Ordering::Relaxed);
        self.anchor_sample.store(anchor_sample, Ordering::Relaxed);
        self.anchor_beat_bits
            .store(anchor_beat.to_bits(), Ordering::Relaxed);
        self.samples_per_quarter_bits
            .store(samples_per_quarter.to_bits(), Ordering::Relaxed);
        self.end_beat_bits
            .store(end_beat.to_bits(), Ordering::Relaxed);
        self.store_loop(loop_window);
        self.open_ended.store(open_ended, Ordering::Relaxed);
        self.current_row.store(row_ordinal as u64, Ordering::Relaxed);
        self.current_row_id.store(row_id.0, Ordering::Relaxed);
        self.ended.store(false, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    fn store_loop(&self, loop_window: Option<(f64, f64)>) {
        let (loop_start, loop_end) = loop_window.unwrap_or((0.0, 0.0));
        self.loop_start_bits.store(loop_start.to_bits(), Ordering::Relaxed);
        self.loop_end_bits.store(loop_end.to_bits(), Ordering::Relaxed);
        self.loop_enabled.store(loop_window.is_some(), Ordering::Relaxed);
    }

    /// A new song's loop brace and end, keeping the published anchor: the
    /// frames rendering now still map from it, only the wrap moves. An
    /// anchor that was a wrap to a start the brace has left is no longer a
    /// wrap of this brace, so beats before it are genuinely before it.
    fn republish_loop(&self, end_beat: f64, loop_window: Option<(f64, f64)>) {
        self.end_beat_bits.store(end_beat.to_bits(), Ordering::Relaxed);
        let anchor = f64::from_bits(self.anchor_beat_bits.load(Ordering::Relaxed));
        if loop_window.is_none_or(|(start, _)| start != anchor) {
            self.anchor_wrapped.store(false, Ordering::Relaxed);
        }
        self.store_loop(loop_window);
    }

    fn mark_ended(&self) {
        self.ended.store(true, Ordering::Release);
    }

    fn clear(&self) {
        self.active.store(false, Ordering::Release);
        self.ended.store(false, Ordering::Relaxed);
    }
}

/// See [`SongPositionShared::render_mapping`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongRenderMapping {
    /// Unwrapped song beat at the block's first sample.
    pub beat: f64,
    pub beats_per_sample: f64,
    pub pass_start_sample: u64,
    pub end_beat: f64,
    /// The beat last published (a row start, a wrap, the pass start).
    pub anchor_beat: f64,
    /// `anchor_beat` is a loop wrap (see [`loop_song_beat`]).
    pub anchor_wrapped: bool,
    /// The loop brace while the current pass loops (see [`loop_song_beat`]).
    pub loop_window: Option<(f64, f64)>,
    pub open_ended: bool,
}

impl SongRenderMapping {
    /// The song beat heard at `sample` (`offset` frames into the block), or
    /// `None` before the pass started or past a non-looping song's end.
    pub fn song_beat(&self, sample: u64, offset: usize) -> Option<f64> {
        if sample < self.pass_start_sample {
            return None;
        }
        let beat = self.beat + offset as f64 * self.beats_per_sample;
        if let Some(window) = self.loop_window {
            Some(loop_song_beat(beat, self.anchor_beat, self.anchor_wrapped, window))
        } else if beat < 0.0 || (beat >= self.end_beat && !self.open_ended) {
            None
        } else {
            Some(beat)
        }
    }
}

/// Bounded command/notice channels plus the shared position atomics. Lives on
/// `SequencerState`, mirroring the `QuantizedLaunchMailbox` pattern.
pub struct SongPlaybackMailbox {
    command_tx: SyncSender<SongPlaybackCommand>,
    command_rx: Mutex<Receiver<SongPlaybackCommand>>,
    notice_tx: SyncSender<SongPlaybackNotice>,
    notice_rx: Mutex<Receiver<SongPlaybackNotice>>,
    /// Set when a notice was dropped because the bounded channel was full.
    /// Slice C capture must fail (never commit an incomplete take) when this
    /// is observed (spec 10.3).
    notice_overflow: AtomicBool,
    shared: SongPositionShared,
}

impl Default for SongPlaybackMailbox {
    fn default() -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(SONG_COMMAND_CAPACITY);
        let (notice_tx, notice_rx) = mpsc::sync_channel(SONG_NOTICE_CAPACITY);
        Self {
            command_tx,
            command_rx: Mutex::new(command_rx),
            notice_tx,
            notice_rx: Mutex::new(notice_rx),
            notice_overflow: AtomicBool::new(false),
            shared: SongPositionShared::default(),
        }
    }
}

impl SongPlaybackMailbox {
    pub fn send_command(&self, command: SongPlaybackCommand) -> Result<(), String> {
        self.command_tx.try_send(command).map_err(|err| match err {
            TrySendError::Full(_) => "song playback command queue is full".to_string(),
            TrySendError::Disconnected(_) => "song playback scheduler disconnected".to_string(),
        })
    }

    /// Scheduler side: drain pending start/stop commands. Never blocks.
    pub fn drain_commands(&self) -> Vec<SongPlaybackCommand> {
        let receiver = self.command_rx.lock().unwrap();
        let mut commands = Vec::new();
        while let Ok(command) = receiver.try_recv() {
            commands.push(command);
        }
        commands
    }

    /// Scheduler side: push one notice. Never blocks; on overflow the notice
    /// is dropped and the sticky overflow flag is set.
    pub fn push_notice(&self, notice: SongPlaybackNotice) {
        match self.notice_tx.try_send(notice) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.notice_overflow.store(true, Ordering::Release);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Control side: drain row-applied/ended notices. Never blocks.
    pub fn drain_notices(&self) -> Vec<SongPlaybackNotice> {
        let receiver = self.notice_rx.lock().unwrap();
        let mut notices = Vec::new();
        while let Ok(notice) = receiver.try_recv() {
            notices.push(notice);
        }
        notices
    }

    /// Sticky overflow flag; reading clears it.
    pub fn take_notice_overflow(&self) -> bool {
        self.notice_overflow.swap(false, Ordering::AcqRel)
    }

    pub fn shared(&self) -> &SongPositionShared {
        &self.shared
    }

    pub fn clear_position(&self) {
        self.shared.clear();
    }
}

/// What the lookahead loop should do next while a song is playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SongChunkPlan {
    /// Schedule `frames` samples (clamped to the next row boundary) from row
    /// `row`'s prebuilt snapshot. `row_changed` is true when one or more row
    /// boundaries were crossed to reach this chunk; `wrapped` when a loop
    /// wrap to song beat zero occurred (the caller rewinds its clock and
    /// self-clocked runtimes to beat zero).
    Schedule {
        frames: usize,
        row: usize,
        row_changed: bool,
        wrapped: bool,
    },
    /// `end_beat` was reached with looping disabled: schedule nothing past
    /// it. The control thread has been notified via the mailbox.
    Ended,
}

/// Scheduler-owned song playback cursor. Owns the current row, boundary
/// detection within lookahead windows, and the exact sample at which each row
/// becomes effective (spec 10.2). All methods are lock-free and
/// allocation-free apart from mailbox notice pushes.
pub struct SongPlaybackRuntime {
    song: Arc<RuntimeSong>,
    /// Latest structural rebuild waiting for the sounding row's next
    /// boundary. Replacing this pointer is allocation-free; every rebuild was
    /// already materialized on the control thread.
    pending_rebuild: Option<Arc<RuntimeSong>>,
    /// The song retained for one returned boundary plan when a pending
    /// rebuild installs. Lookahead uses it to compare the actually sounding
    /// row with the newly governing row for accumulator resets.
    transition_from: Option<(Arc<RuntimeSong>, usize)>,
    samples_per_quarter: f64,
    start_beat: f64,
    initial_row: usize,
    row: usize,
    /// Song beat corresponding to the scheduler clock's beat zero: the start
    /// beat until the first loop wrap, `0.0` afterwards. The song beat at any
    /// planning point is `clock_beat_offset + clock_beats`. The integer
    /// sample origin below owns boundary detection; this offset also maps
    /// the row's authored phase anchors into the scheduler clock domain.
    clock_beat_offset: f64,
    /// Rendered-sample origin for this pass (reset on a loop wrap). Boundary
    /// decisions use integer distance from this origin, never accumulated
    /// floating-point clock error or independently rounded row durations.
    sample_origin: Option<u64>,
    /// Song-timeline beat anchoring the sounding row's lane offsets. Kept
    /// independently from `row` so an identity-compatible cursor remap can
    /// adopt the new layout without moving the phase anchor.
    row_anchor_beat: f64,
    started: bool,
    ended: bool,
    /// See `SongPlaybackCommand::Start::open_ended`.
    open_ended: bool,
    /// A new song (a loop brace edit, say) was installed: the next chunk
    /// republishes the position so the UI and the audio tracks wrap with it
    /// at once, instead of at the next row change.
    republish: bool,
}

impl SongPlaybackRuntime {
    /// `start_beat` is a song-timeline beat; it is loop-normalized when the
    /// song loops and must lie inside `[0, end_beat)` otherwise. V1 callers
    /// pass `0.0` (spec 10.1).
    pub fn new(
        song: Arc<RuntimeSong>,
        start_beat: f64,
        samples_per_quarter: f64,
    ) -> Result<Self, String> {
        if song.rows.is_empty() {
            return Err("runtime song has no rows".to_string());
        }
        if !(samples_per_quarter > 0.0) || !samples_per_quarter.is_finite() {
            return Err(format!(
                "invalid samples-per-quarter {samples_per_quarter} for song playback"
            ));
        }
        let start_beat = song.normalize_start_beat(start_beat)?;
        // Past-end starts (open-ended jam room) are governed by the last
        // row, matching the planner's past-end branch.
        let initial_row = song
            .row_index_at_beat(start_beat)
            .or_else(|| {
                (start_beat >= song.end_beat).then(|| song.rows.len().saturating_sub(1))
            })
            .ok_or_else(|| format!("no song row governs start beat {start_beat}"))?;
        let row_anchor_beat = song.rows[initial_row].start_beat;
        Ok(Self {
            song,
            pending_rebuild: None,
            transition_from: None,
            samples_per_quarter,
            start_beat,
            initial_row,
            row: initial_row,
            clock_beat_offset: start_beat,
            sample_origin: None,
            row_anchor_beat,
            started: false,
            ended: false,
            open_ended: false,
            republish: false,
        })
    }

    pub fn song(&self) -> &Arc<RuntimeSong> {
        &self.song
    }

    /// Opt this runtime out of the song-end stop (spec 7.4). Set by the
    /// scheduler right after `new` when the `Start` command came from
    /// arrangement capture, so `new`'s validation contract is unchanged.
    pub fn set_open_ended(&mut self, open_ended: bool) {
        self.open_ended = open_ended;
    }

    /// Swap in a re-preflighted song without disturbing the cursor (takes
    /// spec 16.7): an edit-through during playback rebuilds the row
    /// snapshots, and the rows that are not yet audible must simply become
    /// correct — no clock reset, no retrigger, no row re-entry. Rejected
    /// when the row layout changed, since the cursor would no longer mean
    /// the same thing; edit-through only ever changes row CONTENT.
    pub fn replace_song_in_place(&mut self, song: Arc<RuntimeSong>) -> bool {
        let same_layout = song.rows.len() == self.song.rows.len()
            && song.end_beat.to_bits() == self.song.end_beat.to_bits()
            && song
                .rows
                .iter()
                .zip(&self.song.rows)
                .all(|(next, prev)| {
                    next.id == prev.id && next.start_beat.to_bits() == prev.start_beat.to_bits()
                });
        if !same_layout {
            return false;
        }
        self.song = song;
        self.republish = true;
        true
    }

    /// Apply a structurally rebuilt song against the scheduler clock's
    /// current planning point. Row ordinals are deliberately ignored: the
    /// governing row is found from the clock-derived song beat.
    ///
    /// Identity-compatible current rows install immediately while retaining
    /// the sounding row's phase anchor. A changed current row stays pending
    /// until `next_chunk` reaches the old sounding row's next boundary. A
    /// rebuild received before the first chunk can install directly because
    /// there is no sounding row to protect.
    pub fn rebuild_song(&mut self, song: Arc<RuntimeSong>, clock_beats: f64) {
        let song_beat = self.clock_beat_offset + clock_beats;
        let next_row = song.row_index_at_beat(song_beat).unwrap_or(0);
        let current = &self.song.rows[self.row];
        let next = &song.rows[next_row];
        let same_sounding_identity = current.resolved_sources == next.resolved_sources
            && current.lane_offsets == next.lane_offsets;
        let past_end = song_beat >= song.end_beat;

        if !self.started || same_sounding_identity || past_end {
            self.install_rebuild(song, next_row, false);
        } else {
            self.pending_rebuild = Some(song);
        }
    }

    fn install_rebuild(
        &mut self,
        song: Arc<RuntimeSong>,
        row: usize,
        at_boundary: bool,
    ) {
        if at_boundary {
            let previous = std::mem::replace(&mut self.song, song);
            self.transition_from = Some((previous, self.row));
            self.row_anchor_beat = self.song.rows[row].start_beat;
        } else {
            self.song = song;
        }
        self.row = row;
        self.initial_row = self
            .song
            .row_index_at_beat(self.start_beat)
            .unwrap_or(0);
        self.pending_rebuild = None;
        self.republish = true;
    }

    pub fn current_row(&self) -> usize {
        self.row
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// The first sample at or after song beat `beat` in this pass: each
    /// authored beat maps independently from the pass's origin, so boundary
    /// decisions never accumulate rounding.
    fn sample_at_beat(&self, beat: f64) -> u64 {
        let origin = self.sample_origin.unwrap_or(0);
        let start_frame = (self.clock_beat_offset * self.samples_per_quarter).ceil() as u64;
        let frame = (beat * self.samples_per_quarter).ceil() as u64;
        origin.saturating_add(frame.saturating_sub(start_frame))
    }

    /// The loop brace playback wraps inside from `at_sample` on: looping is
    /// on and the playhead has not yet passed the loop end. Wraps happen only
    /// when the playhead reaches the end, exactly on its sample, so an edit
    /// that moves the brace takes effect at once and stays in time; a
    /// playhead already past the end (a brace moved behind it, or Play
    /// pressed after it) plays straight on, as in Ableton. A sub-sample brace
    /// never loops, so a wrap always makes progress.
    fn active_loop_window(&self, at_sample: u64) -> Option<(f64, f64)> {
        let (start, end) = self.song.active_loop()?;
        (end > self.clock_beat_offset
            && self.sample_at_beat(end) >= at_sample
            && ((end - start) * self.samples_per_quarter).ceil() >= 1.0)
            .then_some((start, end))
    }

    /// Rewind to the initial start position (transport stopped and will
    /// restart from the song start). Does not touch the mailbox.
    pub fn reset(&mut self) {
        self.row = self.initial_row;
        self.clock_beat_offset = self.start_beat;
        self.sample_origin = None;
        self.row_anchor_beat = self.song.rows[self.initial_row].start_beat;
        self.pending_rebuild = None;
        self.transition_from = None;
        self.started = false;
        self.ended = false;
    }

    pub fn row_snapshot(&self, row: usize) -> Arc<SequencerSnapshot> {
        Arc::clone(&self.song.rows[row].scheduler_snapshot)
    }

    /// The row's per-lane phase anchor in the scheduler clock's beat domain
    /// (takes spec 7.3): the beat at which every lane clip of this row
    /// starts (`row.start_beat` translated by the clock offset, so it stays
    /// correct after a mid-song start or a loop wrap), plus the per-track
    /// clip step offsets.
    pub fn row_clock_anchor(&self, row: usize) -> (f64, &[f64]) {
        let row = &self.song.rows[row];
        (
            self.row_anchor_beat - self.clock_beat_offset,
            &row.lane_offsets,
        )
    }

    /// Rows on either side of the most recent boundary. A deferred Rebuild
    /// crosses between two immutable songs, so the prior row cannot be
    /// recovered by indexing the newly installed layout.
    pub fn transition_rows(
        &self,
        previous_row: usize,
        current_row: usize,
    ) -> (&RuntimeSongRow, &RuntimeSongRow) {
        let previous = self
            .transition_from
            .as_ref()
            .map(|(song, row)| &song.rows[*row])
            .unwrap_or_else(|| &self.song.rows[previous_row]);
        (previous, &self.song.rows[current_row])
    }

    fn publish_position(
        &self,
        mailbox: &SongPlaybackMailbox,
        anchor_sample: u64,
        anchor_beat: f64,
        wrapped: bool,
    ) {
        let row = &self.song.rows[self.row];
        mailbox.shared().publish(
            anchor_sample,
            anchor_beat,
            self.samples_per_quarter,
            self.song.end_beat,
            self.active_loop_window(anchor_sample),
            wrapped,
            self.open_ended,
            self.row,
            row.id,
        );
    }

    /// Plan the next scheduling chunk starting at `at_sample`, where
    /// `clock_beats` is the scheduler clock's accumulated beat position at
    /// that sample. Chunks never cross a row boundary or the song end;
    /// boundary crossings advance the current row, emit
    /// `AudibleSongRowApplied` notices, and (on loop) wrap back to song beat
    /// zero. Each authored boundary maps independently to the first sample
    /// at or after its beat, `ceil(beat * samples_per_quarter)`. Subtracting
    /// integer positions avoids drift from rounding individual row lengths.
    pub fn next_chunk(
        &mut self,
        at_sample: u64,
        clock_beats: f64,
        block: usize,
        mailbox: &SongPlaybackMailbox,
    ) -> SongChunkPlan {
        if self.ended {
            return SongChunkPlan::Ended;
        }
        self.transition_from = None;
        let mut song_beat = self.clock_beat_offset + clock_beats;
        self.sample_origin.get_or_insert_with(|| {
            at_sample.saturating_sub((clock_beats * self.samples_per_quarter).round() as u64)
        });
        if !self.started {
            self.started = true;
            let row = &self.song.rows[self.row];
            mailbox.push_notice(SongPlaybackNotice::RowApplied(AudibleSongRowApplied {
                row_id: row.id,
                row_ordinal: self.row,
                effective_beat: song_beat,
                effective_sample: at_sample,
                wrapped: false,
            }));
            mailbox.shared().mark_pass_start(at_sample);
            self.publish_position(mailbox, at_sample, song_beat, false);
            self.republish = false;
        }
        if std::mem::take(&mut self.republish) {
            mailbox
                .shared()
                .republish_loop(self.song.end_beat, self.active_loop_window(at_sample));
        }

        let mut row_changed = false;
        let mut wrapped = false;
        loop {
            let sample_at = |beat: f64| self.sample_at_beat(beat);
            let (boundary_beat, boundary_is_end) = match self.song.rows.get(self.row + 1) {
                Some(next) => (next.start_beat, false),
                None => (self.song.end_beat, true),
            };
            let remaining_samples = sample_at(boundary_beat).saturating_sub(at_sample);
            let loop_window = self.active_loop_window(at_sample);
            let loop_remaining =
                loop_window.map(|(_, end)| sample_at(end).saturating_sub(at_sample));
            let at_loop_end = loop_remaining == Some(0);
            if remaining_samples > 0 && !at_loop_end {
                let frames = (block as u64)
                    .min(remaining_samples)
                    .min(loop_remaining.unwrap_or(u64::MAX)) as usize;
                return SongChunkPlan::Schedule {
                    frames,
                    row: self.row,
                    row_changed,
                    wrapped,
                };
            }
            if let Some(song) = self.pending_rebuild.take() {
                let next_row = song.row_index_at_beat(song_beat).unwrap_or(0);
                let past_end = song_beat >= song.end_beat;
                self.install_rebuild(song, next_row, true);
                row_changed = true;
                // At the loop end the wrap below re-enters a row and
                // publishes on this same sample: one notice, not two.
                if !at_loop_end && (!past_end || self.open_ended) {
                    let row = &self.song.rows[self.row];
                    mailbox.push_notice(SongPlaybackNotice::RowApplied(
                        AudibleSongRowApplied {
                            row_id: row.id,
                            row_ordinal: self.row,
                            effective_beat: song_beat,
                            effective_sample: at_sample,
                            wrapped: false,
                        },
                    ));
                    self.publish_position(mailbox, at_sample, song_beat, false);
                }
                // Re-evaluate against the rebuilt layout. If the new end is
                // already behind the cursor, the ordinary end/loop branch
                // below now owns the outcome.
                continue;
            }
            if let Some((loop_start, _)) = loop_window.filter(|_| at_loop_end) {
                // Wrap to the loop start: the caller rewinds its clock to
                // beat zero for the chunk starting at this sample, which is
                // song beat `loop_start` from here on.
                self.clock_beat_offset = loop_start;
                self.sample_origin = Some(at_sample);
                song_beat = loop_start;
                self.row = self.song.row_index_at_beat(loop_start).unwrap_or(0);
                self.row_anchor_beat = self.song.rows[self.row].start_beat;
                row_changed = true;
                wrapped = true;
                let row = &self.song.rows[self.row];
                mailbox.push_notice(SongPlaybackNotice::RowApplied(AudibleSongRowApplied {
                    row_id: row.id,
                    row_ordinal: self.row,
                    effective_beat: loop_start,
                    effective_sample: at_sample,
                    wrapped: true,
                }));
                self.publish_position(mailbox, at_sample, loop_start, true);
                continue;
            }
            if boundary_is_end {
                if self.open_ended || loop_window.is_some() {
                    // Past the song end the last row simply keeps playing:
                    // recording past the old end (spec 7.4), or a loop brace
                    // that reaches beyond it. `song_beat` runs past
                    // `end_beat` from here, so this branch is re-entered
                    // every chunk and schedules up to the loop end, if any.
                    let frames = (block as u64).min(loop_remaining.unwrap_or(u64::MAX)) as usize;
                    return SongChunkPlan::Schedule {
                        frames,
                        row: self.row,
                        row_changed,
                        wrapped,
                    };
                }
                self.ended = true;
                mailbox.push_notice(SongPlaybackNotice::Ended {
                    end_beat: self.song.end_beat,
                    end_sample: at_sample,
                });
                mailbox.shared().mark_ended();
                return SongChunkPlan::Ended;
            }
            self.row += 1;
            self.row_anchor_beat = self.song.rows[self.row].start_beat;
            row_changed = true;
            let row = &self.song.rows[self.row];
            mailbox.push_notice(SongPlaybackNotice::RowApplied(AudibleSongRowApplied {
                row_id: row.id,
                row_ordinal: self.row,
                effective_beat: row.start_beat,
                effective_sample: at_sample,
                wrapped: false,
            }));
            self.publish_position(mailbox, at_sample, row.start_beat, false);
        }
    }
}
