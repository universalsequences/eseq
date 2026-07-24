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
    pub scene: usize,
    /// The row's complete override set as live pool ids (`None` =
    /// explicit-empty: the track is silenced for the row).
    pub overrides: Vec<(usize, Option<PatternId>)>,
    /// Effective pattern per track: override, else scene cell, else `None`
    /// (the track is scene-silenced for this row).
    pub resolved_pattern_ids: Vec<Option<PatternId>>,
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
    /// True when this application came from a loop wrap back to beat zero.
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
    anchor_sample: AtomicU64,
    anchor_beat_bits: AtomicU64,
    samples_per_quarter_bits: AtomicU64,
    end_beat_bits: AtomicU64,
    current_row: AtomicU64,
    current_row_id: AtomicU64,
}

impl Default for SongPositionShared {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            loop_enabled: AtomicBool::new(false),
            anchor_sample: AtomicU64::new(0),
            anchor_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
            samples_per_quarter_bits: AtomicU64::new(0.0_f64.to_bits()),
            end_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
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
        if self.loop_enabled.load(Ordering::Relaxed) && end_beat > 0.0 {
            Some(beats.rem_euclid(end_beat))
        } else {
            Some(beats.clamp(0.0, end_beat))
        }
    }

    fn publish(
        &self,
        anchor_sample: u64,
        anchor_beat: f64,
        samples_per_quarter: f64,
        end_beat: f64,
        loop_enabled: bool,
        row_ordinal: usize,
        row_id: SongRowId,
    ) {
        self.anchor_sample.store(anchor_sample, Ordering::Relaxed);
        self.anchor_beat_bits
            .store(anchor_beat.to_bits(), Ordering::Relaxed);
        self.samples_per_quarter_bits
            .store(samples_per_quarter.to_bits(), Ordering::Relaxed);
        self.end_beat_bits
            .store(end_beat.to_bits(), Ordering::Relaxed);
        self.loop_enabled.store(loop_enabled, Ordering::Relaxed);
        self.current_row.store(row_ordinal as u64, Ordering::Relaxed);
        self.current_row_id.store(row_id.0, Ordering::Relaxed);
        self.ended.store(false, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    fn mark_ended(&self) {
        self.ended.store(true, Ordering::Release);
    }

    fn clear(&self) {
        self.active.store(false, Ordering::Release);
        self.ended.store(false, Ordering::Relaxed);
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
    samples_per_quarter: f64,
    start_beat: f64,
    initial_row: usize,
    row: usize,
    /// Song beat corresponding to the scheduler clock's beat zero: the start
    /// beat until the first loop wrap, `0.0` afterwards. The song beat at any
    /// planning point is `clock_beat_offset + clock_beats`, so boundary
    /// detection follows the production clock's own beat accumulation and a
    /// pattern step coinciding with a row boundary is scheduled from the new
    /// row at exactly the sample where the clock crosses the boundary beat.
    clock_beat_offset: f64,
    started: bool,
    ended: bool,
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
        if !start_beat.is_finite() || start_beat < 0.0 {
            return Err(format!("invalid song start beat {start_beat}"));
        }
        let start_beat = if song.loop_enabled {
            if song.end_beat > 0.0 {
                start_beat.rem_euclid(song.end_beat)
            } else {
                return Err("song end beat must be positive".to_string());
            }
        } else {
            if start_beat >= song.end_beat {
                return Err(format!(
                    "song start beat {start_beat} is at or past the song end {}",
                    song.end_beat
                ));
            }
            start_beat
        };
        let initial_row = song
            .row_index_at_beat(start_beat)
            .ok_or_else(|| format!("no song row governs start beat {start_beat}"))?;
        Ok(Self {
            song,
            samples_per_quarter,
            start_beat,
            initial_row,
            row: initial_row,
            clock_beat_offset: start_beat,
            started: false,
            ended: false,
        })
    }

    pub fn song(&self) -> &Arc<RuntimeSong> {
        &self.song
    }

    pub fn current_row(&self) -> usize {
        self.row
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Rewind to the initial start position (transport stopped and will
    /// restart from the song start). Does not touch the mailbox.
    pub fn reset(&mut self) {
        self.row = self.initial_row;
        self.clock_beat_offset = self.start_beat;
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
        (row.start_beat - self.clock_beat_offset, &row.lane_offsets)
    }

    fn publish_position(&self, mailbox: &SongPlaybackMailbox, anchor_sample: u64, anchor_beat: f64) {
        let row = &self.song.rows[self.row];
        mailbox.shared().publish(
            anchor_sample,
            anchor_beat,
            self.samples_per_quarter,
            self.song.end_beat,
            self.song.loop_enabled,
            self.row,
            row.id,
        );
    }

    /// Plan the next scheduling chunk starting at `at_sample`, where
    /// `clock_beats` is the scheduler clock's accumulated beat position at
    /// that sample. Chunks never cross a row boundary or the song end;
    /// boundary crossings advance the current row, emit
    /// `AudibleSongRowApplied` notices, and (on loop) wrap back to song beat
    /// zero. Boundaries are located by the clock's own beat accumulation and
    /// map to samples without snapping to any musical grid (spec 8.2): the
    /// row becomes effective at the exact sample where the clock reaches its
    /// start beat.
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
        let mut song_beat = self.clock_beat_offset + clock_beats;
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
            self.publish_position(mailbox, at_sample, song_beat);
        }

        let mut row_changed = false;
        let mut wrapped = false;
        loop {
            let (boundary_beat, boundary_is_end) = match self.song.rows.get(self.row + 1) {
                Some(next) => (next.start_beat, false),
                None => (self.song.end_beat, true),
            };
            let remaining_samples = (boundary_beat - song_beat) * self.samples_per_quarter;
            if remaining_samples >= 1.0 {
                let frames = block.min(remaining_samples.floor() as usize).max(1);
                return SongChunkPlan::Schedule {
                    frames,
                    row: self.row,
                    row_changed,
                    wrapped,
                };
            }
            if boundary_is_end {
                // Guard against a degenerate sub-sample song hanging the
                // wrap loop: a wrap must make forward progress.
                if self.song.loop_enabled
                    && self.song.end_beat * self.samples_per_quarter >= 1.0
                {
                    // Wrap to song beat zero: the caller rewinds its clock to
                    // beat zero for the chunk starting at this sample.
                    self.clock_beat_offset = 0.0;
                    song_beat = 0.0;
                    self.row = self.song.row_index_at_beat(0.0).unwrap_or(0);
                    row_changed = true;
                    wrapped = true;
                    let row = &self.song.rows[self.row];
                    mailbox.push_notice(SongPlaybackNotice::RowApplied(AudibleSongRowApplied {
                        row_id: row.id,
                        row_ordinal: self.row,
                        effective_beat: 0.0,
                        effective_sample: at_sample,
                        wrapped: true,
                    }));
                    self.publish_position(mailbox, at_sample, 0.0);
                    continue;
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
            row_changed = true;
            let row = &self.song.rows[self.row];
            mailbox.push_notice(SongPlaybackNotice::RowApplied(AudibleSongRowApplied {
                row_id: row.id,
                row_ordinal: self.row,
                effective_beat: row.start_beat,
                effective_sample: at_sample,
                wrapped: false,
            }));
            self.publish_position(mailbox, at_sample, row.start_beat);
        }
    }
}
