use crate::sequencer::SequencerSnapshot;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};

pub type QuantizedLaunchToken = u64;

const REQUEST_CAPACITY: usize = 256;
const DUE_CAPACITY: usize = 256;
const BOUNDARY_EPSILON_BEATS: f64 = 1.0e-9;
pub const QUARTER_NOTES_PER_BAR: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchQuantize {
    Off,
    Sixteenth,
    Eighth,
    Quarter,
    Half,
    Bar,
}

impl LaunchQuantize {
    pub fn from_transport_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "1/16" => Some(Self::Sixteenth),
            "1/8" => Some(Self::Eighth),
            "1/4" => Some(Self::Quarter),
            "1/2" => Some(Self::Half),
            "1 bar" | "bar" => Some(Self::Bar),
            _ => None,
        }
    }

    pub fn transport_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Sixteenth => "1/16",
            Self::Eighth => "1/8",
            Self::Quarter => "1/4",
            Self::Half => "1/2",
            Self::Bar => "1 bar",
        }
    }

    fn grid_beats(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::Sixteenth => Some(0.25),
            Self::Eighth => Some(0.5),
            Self::Quarter => Some(1.0),
            Self::Half => Some(2.0),
            Self::Bar => Some(QUARTER_NOTES_PER_BAR),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternLaunchTarget {
    Scene { scene: usize },
    SceneTracks { scene: usize, tracks: Vec<usize> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuantizedLaunchOwner {
    SceneMacro(u32),
    Transport,
}

#[derive(Clone, Debug)]
pub struct QuantizedLaunchRequest {
    pub token: QuantizedLaunchToken,
    pub target: PatternLaunchTarget,
    pub quantize: LaunchQuantize,
    pub owner: QuantizedLaunchOwner,
    /// Prebuilt scheduler snapshot of the launch target (the song-row
    /// preflight pattern, docs/song-mode-spec.md 9). When present and the
    /// transport is playing in session mode, the scheduler applies the
    /// launch itself by splitting the lookahead chunk exactly at the
    /// quantize boundary and scheduling at/after it from this snapshot — no
    /// epoch bump, no queue clear, no missed first-step triggers. `None`
    /// falls back to the control-side apply once the boundary has rendered.
    pub snapshot: Option<Arc<SequencerSnapshot>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DuePatternLaunch {
    pub token: QuantizedLaunchToken,
    pub target: PatternLaunchTarget,
    /// The scheduler-stamped audible deadline in rendered beats: the exact
    /// grid boundary for quantized launches, or the rendered-beat position at
    /// scheduling time for unquantized ones. Song capture stores this beat
    /// (docs/song-mode-spec.md 8.3), never the drain/request time.
    pub deadline_beats: f64,
    /// True when the scheduler already made this launch audible at the
    /// boundary from its prebuilt snapshot. The control thread must MIRROR
    /// it (no pattern-epoch bump — a bump would drop the in-flight scheduled
    /// events, exactly like the song-row mirror) and then acknowledge with
    /// `acknowledge_mirror` so the scheduler releases its snapshot override.
    pub scheduler_applied: bool,
}

#[derive(Clone, Debug)]
pub enum QuantizedLaunchMessage {
    Schedule(QuantizedLaunchRequest),
    CancelToken(QuantizedLaunchToken),
    CancelOwner(QuantizedLaunchOwner),
    CancelAll,
    /// Control-side mirror of a scheduler-applied boundary launch finished:
    /// the base snapshot now carries the launched content, so the scheduler
    /// drops its per-chunk snapshot override.
    BoundaryMirrored(QuantizedLaunchToken),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuantizedLaunchSubmitError {
    EmptyTrackMask,
    SceneOutOfRange { scene: usize, scene_count: usize },
    TrackOutOfRange { track: usize, track_count: usize },
    RequestQueueFull,
    SchedulerDisconnected,
}

#[derive(Clone, Debug)]
struct PendingLaunch {
    request: QuantizedLaunchRequest,
    deadline_beats: f64,
}

#[derive(Clone, Debug)]
struct PendingDueLaunch {
    action: DuePatternLaunch,
    owner: QuantizedLaunchOwner,
}

/// A quantized launch the lookahead pass applies itself: the chunk is
/// clamped at `deadline_beats` and scheduling at/after that boundary comes
/// from the request's prebuilt snapshot (docs/song-mode-spec.md 10.2 chunk
/// split, applied to session launches).
#[derive(Clone, Debug)]
struct PendingBoundaryLaunch {
    request: QuantizedLaunchRequest,
    deadline_beats: f64,
}

/// A boundary launch the scheduler has made audible but the control thread
/// has not yet mirrored into the base snapshot. Chunks keep scheduling from
/// this override until `BoundaryMirrored` arrives.
struct InstalledBoundaryLaunch {
    token: QuantizedLaunchToken,
    snapshot: Arc<SequencerSnapshot>,
    /// `None` = full scene launch; `Some` = the launched track mask (other
    /// tracks keep scheduling from the base snapshot via a per-chunk merge).
    tracks: Option<Vec<usize>>,
}

/// A scene index an installed boundary launch will publish as
/// `current_pattern` once the control thread mirrors it.
struct AdoptedPattern {
    token: QuantizedLaunchToken,
    pattern: usize,
}

/// Adoption expectations outlive their install (see `adopted_patterns`), so
/// cap the bookkeeping in case a mirror never lands.
const MAX_ADOPTED_PATTERNS: usize = 8;

/// Which tracks a just-installed boundary launch replaced — the lookahead
/// pass marks their accumulators for reset, mirroring the control-side
/// pattern-switch behavior.
pub(crate) enum SessionLaunchInstall {
    None,
    AllTracks,
    Tracks(Vec<usize>),
}

#[derive(Default)]
pub(crate) struct PendingQuantizedLaunches {
    pending: HashMap<QuantizedLaunchToken, PendingLaunch>,
    owner_tokens: HashMap<QuantizedLaunchOwner, QuantizedLaunchToken>,
    due_backlog: Vec<PendingDueLaunch>,
    boundary: Vec<PendingBoundaryLaunch>,
    /// Every boundary launch made audible but not yet mirrored, in install
    /// order. Concurrent owners can land on one boundary (a scene macro plus
    /// a transport track launch), and each due is reported as
    /// `scheduler_applied`, so all of their snapshots must stay in the
    /// per-chunk override — a single slot would silently drop the earlier
    /// launch's content while still telling the control thread it sounded.
    installed: Vec<InstalledBoundaryLaunch>,
    /// Bumped whenever `installed` changes; keys the merge cache.
    install_revision: u64,
    /// Scene indices the pending mirrors will publish as `current_pattern`,
    /// in mirror order. Deliberately NOT released by `BoundaryMirrored`: the
    /// control thread publishes the mirrored snapshot BEFORE it acks, and the
    /// worker drains the ack earlier in its loop than it compares patterns —
    /// releasing on the ack would let the pattern-switch resync fire on the
    /// mirror and undo the boundary split. Released when the worker actually
    /// observes the published pattern change.
    adopted_patterns: Vec<AdoptedPattern>,
    /// Cached snapshot override keyed by (install revision, base snapshot
    /// address) so per-chunk merges don't reallocate every block.
    merged_cache: Option<(u64, usize, Arc<SequencerSnapshot>)>,
}

impl PendingQuantizedLaunches {
    pub(crate) fn process(
        &mut self,
        request_rx: &Receiver<QuantizedLaunchMessage>,
        due_tx: &SyncSender<DuePatternLaunch>,
        rendered_beats: f64,
        frontier_beats: f64,
        playing: bool,
        song_active: bool,
    ) {
        // The merge cache is keyed by the base snapshot's address, which is
        // only guaranteed stable within one worker iteration — a republished
        // snapshot could reuse a freed allocation across iterations.
        self.merged_cache = None;
        loop {
            match request_rx.try_recv() {
                Ok(message) => self.handle_message(
                    message,
                    rendered_beats,
                    frontier_beats,
                    playing,
                    song_active,
                ),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }

        // Boundary launches only exist while session playback runs: on stop
        // (or a song taking launch authority) they degrade to the legacy
        // control-side path, which emits immediately while stopped and is
        // dropped by the control drain during song playback.
        if !playing || song_active {
            for launch in self.boundary.drain(..) {
                self.pending.insert(
                    launch.request.token,
                    PendingLaunch {
                        deadline_beats: rendered_beats,
                        request: launch.request,
                    },
                );
            }
            self.clear_installed();
        }

        let mut due_tokens = self
            .pending
            .iter()
            .filter_map(|(token, pending)| {
                (pending.deadline_beats <= rendered_beats + BOUNDARY_EPSILON_BEATS)
                    .then_some(*token)
            })
            .collect::<Vec<_>>();
        // Tokens are monotonic submission order. Preserve that order at a
        // shared boundary so the last scheduled owner deterministically wins.
        due_tokens.sort_unstable();
        for token in due_tokens {
            if let Some(pending) = self.pending.remove(&token) {
                if self.owner_tokens.get(&pending.request.owner) == Some(&token) {
                    self.owner_tokens.remove(&pending.request.owner);
                }
                self.due_backlog.push(PendingDueLaunch {
                    action: DuePatternLaunch {
                        token,
                        target: pending.request.target,
                        deadline_beats: pending.deadline_beats,
                        scheduler_applied: false,
                    },
                    owner: pending.request.owner,
                });
            }
        }

        let mut sent = 0;
        while sent < self.due_backlog.len() {
            match due_tx.try_send(self.due_backlog[sent].action.clone()) {
                Ok(()) => sent += 1,
                Err(TrySendError::Full(_)) => break,
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        self.due_backlog.drain(..sent);
    }

    fn handle_message(
        &mut self,
        message: QuantizedLaunchMessage,
        rendered_beats: f64,
        frontier_beats: f64,
        playing: bool,
        song_active: bool,
    ) {
        match message {
            QuantizedLaunchMessage::Schedule(request) => {
                if let Some(replaced) = self.owner_tokens.insert(request.owner, request.token) {
                    self.pending.remove(&replaced);
                    self.boundary
                        .retain(|launch| launch.request.token != replaced);
                }
                if playing
                    && !song_active
                    && request.quantize != LaunchQuantize::Off
                    && request.snapshot.is_some()
                {
                    // Scheduler-applied boundary launch: the deadline is the
                    // next grid boundary after the SCHEDULING frontier, so
                    // the boundary is guaranteed not to be scheduled yet and
                    // the chunk split lands cleanly (mirrors song rows). The
                    // frontier runs ahead of the rendered position by the
                    // lookahead window, so this occasionally lands one grid
                    // slot later than the strictly-next audible boundary —
                    // never earlier.
                    let deadline_beats =
                        launch_deadline(frontier_beats, playing, request.quantize);
                    self.boundary.push(PendingBoundaryLaunch {
                        request,
                        deadline_beats,
                    });
                } else {
                    let deadline_beats =
                        launch_deadline(rendered_beats, playing, request.quantize);
                    self.pending.insert(
                        request.token,
                        PendingLaunch {
                            request,
                            deadline_beats,
                        },
                    );
                }
            }
            QuantizedLaunchMessage::CancelToken(token) => self.cancel_token(token),
            QuantizedLaunchMessage::CancelOwner(owner) => {
                if let Some(token) = self.owner_tokens.remove(&owner) {
                    self.pending.remove(&token);
                    self.boundary.retain(|launch| launch.request.token != token);
                    self.drop_installed_token(token);
                    self.forget_adopted_token(token);
                }
                self.boundary.retain(|launch| launch.request.owner != owner);
                self.due_backlog.retain(|due| due.owner != owner);
            }
            QuantizedLaunchMessage::CancelAll => {
                self.pending.clear();
                self.owner_tokens.clear();
                self.due_backlog.clear();
                self.boundary.clear();
                self.clear_installed();
            }
            QuantizedLaunchMessage::BoundaryMirrored(token) => {
                self.drop_installed_token(token);
            }
        }
    }

    fn cancel_token(&mut self, token: QuantizedLaunchToken) {
        if let Some(pending) = self.pending.remove(&token) {
            if self.owner_tokens.get(&pending.request.owner) == Some(&token) {
                self.owner_tokens.remove(&pending.request.owner);
            }
        }
        if let Some(idx) = self
            .boundary
            .iter()
            .position(|launch| launch.request.token == token)
        {
            let launch = self.boundary.remove(idx);
            if self.owner_tokens.get(&launch.request.owner) == Some(&token) {
                self.owner_tokens.remove(&launch.request.owner);
            }
        }
        self.drop_installed_token(token);
        // A cancelled launch never reaches the control thread, so its adoption
        // expectation would never be observed — drop it with the install.
        self.forget_adopted_token(token);
        self.due_backlog.retain(|due| due.action.token != token);
    }

    fn drop_installed_token(&mut self, token: QuantizedLaunchToken) {
        let before = self.installed.len();
        self.installed.retain(|installed| installed.token != token);
        if self.installed.len() != before {
            self.install_revision = self.install_revision.wrapping_add(1);
            self.merged_cache = None;
        }
    }

    fn forget_adopted_token(&mut self, token: QuantizedLaunchToken) {
        self.adopted_patterns
            .retain(|adopted| adopted.token != token);
    }

    fn clear_installed(&mut self) {
        if !self.installed.is_empty() {
            self.installed.clear();
            self.install_revision = self.install_revision.wrapping_add(1);
        }
        self.adopted_patterns.clear();
        self.merged_cache = None;
    }

    /// Plan the next session-mode scheduling chunk: clamp it to the earliest
    /// pending boundary launch, and when the frontier stands on a boundary,
    /// install that launch so the chunk starting at this exact sample
    /// schedules from the launched snapshot (song-row chunk split semantics,
    /// docs/song-mode-spec.md 10.2). Returns the chunk frame count and which
    /// tracks were switched (for accumulator resets).
    pub(crate) fn next_session_chunk(
        &mut self,
        clock_beats: f64,
        samples_per_quarter: f64,
        block: usize,
    ) -> (usize, SessionLaunchInstall) {
        let mut install = SessionLaunchInstall::None;
        loop {
            // Earliest deadline; ties resolve in token order so the last
            // scheduled owner deterministically wins a shared boundary.
            let Some(idx) = self
                .boundary
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    (a.deadline_beats, a.request.token)
                        .partial_cmp(&(b.deadline_beats, b.request.token))
                        .expect("launch deadlines are finite")
                })
                .map(|(idx, _)| idx)
            else {
                return (block, install);
            };
            let remaining_samples =
                (self.boundary[idx].deadline_beats - clock_beats) * samples_per_quarter;
            if remaining_samples >= 1.0 {
                let frames = block.min(remaining_samples.floor() as usize).max(1);
                return (frames, install);
            }
            let launch = self.boundary.remove(idx);
            let token = launch.request.token;
            if self.owner_tokens.get(&launch.request.owner) == Some(&token) {
                self.owner_tokens.remove(&launch.request.owner);
            }
            let snapshot = launch
                .request
                .snapshot
                .clone()
                .expect("boundary launches always carry a prebuilt snapshot");
            let (tracks, expected_pattern) = match &launch.request.target {
                PatternLaunchTarget::Scene { scene } => (None, Some(*scene)),
                PatternLaunchTarget::SceneTracks { tracks, .. } => {
                    (Some(tracks.clone()), None)
                }
            };
            install = match (&install, &tracks) {
                (SessionLaunchInstall::AllTracks, _) | (_, None) => {
                    SessionLaunchInstall::AllTracks
                }
                (SessionLaunchInstall::None, Some(new)) => {
                    SessionLaunchInstall::Tracks(new.clone())
                }
                (SessionLaunchInstall::Tracks(prev), Some(new)) => {
                    let mut merged = prev.clone();
                    merged.extend(new.iter().copied());
                    merged.sort_unstable();
                    merged.dedup();
                    SessionLaunchInstall::Tracks(merged)
                }
            };
            if tracks.is_none() {
                // A full-scene launch replaces every track, so the overrides
                // installed before it on this boundary no longer contribute.
                self.installed.clear();
            }
            self.installed.push(InstalledBoundaryLaunch {
                token,
                snapshot,
                tracks,
            });
            self.install_revision = self.install_revision.wrapping_add(1);
            self.merged_cache = None;
            if let Some(pattern) = expected_pattern {
                if self.adopted_patterns.len() >= MAX_ADOPTED_PATTERNS {
                    self.adopted_patterns.remove(0);
                }
                self.adopted_patterns.push(AdoptedPattern { token, pattern });
            }
            self.due_backlog.push(PendingDueLaunch {
                action: DuePatternLaunch {
                    token,
                    target: launch.request.target,
                    deadline_beats: launch.deadline_beats,
                    scheduler_applied: true,
                },
                owner: launch.request.owner,
            });
        }
    }

    /// The snapshot the current session chunk should schedule from: the
    /// installed boundary launch's prebuilt snapshot (full-scene launches),
    /// or the base snapshot with the launched tracks swapped in (track-mask
    /// launches — same merge shape as the manual-override latch). `None`
    /// when no boundary launch is awaiting its control-side mirror.
    pub(crate) fn session_snapshot(
        &mut self,
        base_snapshot: &SequencerSnapshot,
    ) -> Option<Arc<SequencerSnapshot>> {
        if self.installed.is_empty() {
            return None;
        }
        let base_key = base_snapshot as *const SequencerSnapshot as usize;
        if let Some((revision, key, merged)) = &self.merged_cache {
            if *revision == self.install_revision && *key == base_key {
                return Some(Arc::clone(merged));
            }
        }
        // Fast path: a lone full-scene launch whose preflight transport still
        // agrees with the live one is usable verbatim.
        if let [only] = self.installed.as_slice() {
            if only.tracks.is_none() && live_transport_matches(&only.snapshot, base_snapshot) {
                return Some(Arc::clone(&only.snapshot));
            }
        }
        let mut merged = base_snapshot.clone();
        for installed in &self.installed {
            match &installed.tracks {
                None => merged = (*installed.snapshot).clone(),
                Some(tracks) => {
                    let track_count = merged.tracks.len().min(installed.snapshot.tracks.len());
                    for &track in tracks {
                        if track < track_count {
                            merged.tracks[track] = Arc::clone(&installed.snapshot.tracks[track]);
                        }
                    }
                }
            }
        }
        // The prebuilt snapshot froze bpm/topology/track count at preflight,
        // which can be a whole quantize interval before the boundary. The
        // clock derives its beat rate from the chunk snapshot while the
        // surrounding chunk math uses the live base snapshot, so the live
        // transport has to win for the override window.
        adopt_live_transport(&mut merged, base_snapshot);
        let merged = Arc::new(merged);
        self.merged_cache = Some((self.install_revision, base_key, Arc::clone(&merged)));
        Some(merged)
    }

    /// Does a published `current_pattern` match a boundary launch the
    /// scheduler already made audible? The worker skips its pattern-switch
    /// resync when it does — the switch happened at the chunk split, and a
    /// queue clear + seek here would swallow the boundary step (the original
    /// skipped-first-trigger bug).
    ///
    /// Observing the switch consumes the expectation (and any superseded ones
    /// ahead of it); an unrelated pattern voids the pending expectations
    /// rather than letting them suppress a later legitimate resync.
    pub(crate) fn observe_pattern_switch(&mut self, pattern: usize) -> bool {
        match self
            .adopted_patterns
            .iter()
            .position(|adopted| adopted.pattern == pattern)
        {
            Some(index) => {
                self.adopted_patterns.drain(..=index);
                true
            }
            None => {
                self.adopted_patterns.clear();
                false
            }
        }
    }

    /// Scene index the next pending mirror will publish as `current_pattern`,
    /// for tests and diagnostics.
    pub(crate) fn adopted_pattern(&self) -> Option<usize> {
        self.adopted_patterns.first().map(|adopted| adopted.pattern)
    }
}

fn live_transport_matches(snapshot: &SequencerSnapshot, base: &SequencerSnapshot) -> bool {
    snapshot.transport.bpm == base.transport.bpm
        && snapshot.transport.topology_epoch == base.transport.topology_epoch
        && snapshot.transport.num_tracks == live_track_count(snapshot, base)
}

fn adopt_live_transport(snapshot: &mut SequencerSnapshot, base: &SequencerSnapshot) {
    snapshot.transport.bpm = base.transport.bpm;
    snapshot.transport.topology_epoch = base.transport.topology_epoch;
    snapshot.transport.num_tracks = live_track_count(snapshot, base);
}

/// The live track count, clamped to what the override snapshot actually
/// carries — the clock indexes `tracks[0..num_tracks]` directly.
fn live_track_count(snapshot: &SequencerSnapshot, base: &SequencerSnapshot) -> usize {
    base.transport.num_tracks.min(snapshot.tracks.len())
}

pub(crate) fn launch_deadline(rendered_beats: f64, playing: bool, quantize: LaunchQuantize) -> f64 {
    if !playing || quantize == LaunchQuantize::Off {
        return rendered_beats;
    }
    let grid = quantize
        .grid_beats()
        .expect("non-off launch quantization must define a beat grid");
    ((rendered_beats + BOUNDARY_EPSILON_BEATS) / grid)
        .floor()
        .mul_add(grid, grid)
}

pub struct QuantizedLaunchMailbox {
    request_tx: SyncSender<QuantizedLaunchMessage>,
    request_rx: Mutex<Receiver<QuantizedLaunchMessage>>,
    due_tx: SyncSender<DuePatternLaunch>,
    due_rx: Mutex<Receiver<DuePatternLaunch>>,
    next_token: AtomicU64,
    valid_tokens: Mutex<HashSet<QuantizedLaunchToken>>,
    owner_tokens: Mutex<HashMap<QuantizedLaunchOwner, QuantizedLaunchToken>>,
    owner_targets: Mutex<HashMap<QuantizedLaunchOwner, PatternLaunchTarget>>,
}

impl Default for QuantizedLaunchMailbox {
    fn default() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (due_tx, due_rx) = mpsc::sync_channel(DUE_CAPACITY);
        Self {
            request_tx,
            request_rx: Mutex::new(request_rx),
            due_tx,
            due_rx: Mutex::new(due_rx),
            next_token: AtomicU64::new(1),
            valid_tokens: Mutex::new(HashSet::new()),
            owner_tokens: Mutex::new(HashMap::new()),
            owner_targets: Mutex::new(HashMap::new()),
        }
    }
}

impl QuantizedLaunchMailbox {
    pub fn schedule(
        &self,
        target: PatternLaunchTarget,
        quantize: LaunchQuantize,
        owner: QuantizedLaunchOwner,
        scene_count: usize,
        track_count: usize,
        snapshot: Option<Arc<SequencerSnapshot>>,
    ) -> Result<QuantizedLaunchToken, QuantizedLaunchSubmitError> {
        let target = canonicalize_target(target, scene_count, track_count)?;
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let request = QuantizedLaunchRequest {
            token,
            target: target.clone(),
            quantize,
            owner,
            snapshot,
        };
        self.request_tx
            .try_send(QuantizedLaunchMessage::Schedule(request))
            .map_err(map_send_error)?;

        let mut owners = self.owner_tokens.lock().unwrap();
        let mut valid = self.valid_tokens.lock().unwrap();
        if let Some(replaced) = owners.insert(owner, token) {
            valid.remove(&replaced);
        }
        self.owner_targets.lock().unwrap().insert(owner, target);
        valid.insert(token);
        Ok(token)
    }

    pub fn cancel_token(
        &self,
        token: QuantizedLaunchToken,
    ) -> Result<(), QuantizedLaunchSubmitError> {
        let removed_owners = {
            let mut owners = self.owner_tokens.lock().unwrap();
            let removed = owners
                .iter()
                .filter_map(|(owner, value)| (*value == token).then_some(*owner))
                .collect::<Vec<_>>();
            owners.retain(|_, value| *value != token);
            removed
        };
        let mut targets = self.owner_targets.lock().unwrap();
        for owner in removed_owners {
            targets.remove(&owner);
        }
        drop(targets);
        self.valid_tokens.lock().unwrap().remove(&token);
        self.request_tx
            .try_send(QuantizedLaunchMessage::CancelToken(token))
            .map_err(map_send_error)
    }

    pub fn cancel_owner(
        &self,
        owner: QuantizedLaunchOwner,
    ) -> Result<(), QuantizedLaunchSubmitError> {
        if let Some(token) = self.owner_tokens.lock().unwrap().remove(&owner) {
            self.valid_tokens.lock().unwrap().remove(&token);
        }
        self.owner_targets.lock().unwrap().remove(&owner);
        self.request_tx
            .try_send(QuantizedLaunchMessage::CancelOwner(owner))
            .map_err(map_send_error)
    }

    pub fn cancel_all(&self) -> Result<(), QuantizedLaunchSubmitError> {
        self.valid_tokens.lock().unwrap().clear();
        self.owner_tokens.lock().unwrap().clear();
        self.owner_targets.lock().unwrap().clear();
        self.request_tx
            .try_send(QuantizedLaunchMessage::CancelAll)
            .map_err(map_send_error)
    }

    /// Control side: the mirror of a scheduler-applied boundary launch has
    /// published the base snapshot — tell the scheduler to drop its snapshot
    /// override. Sent for every scheduler-applied due the control thread
    /// consumed, even when the mirror errored, so the override never leaks.
    pub fn acknowledge_mirror(
        &self,
        token: QuantizedLaunchToken,
    ) -> Result<(), QuantizedLaunchSubmitError> {
        self.request_tx
            .try_send(QuantizedLaunchMessage::BoundaryMirrored(token))
            .map_err(map_send_error)
    }

    pub fn drain_valid_due(&self) -> Vec<DuePatternLaunch> {
        let receiver = self.due_rx.lock().unwrap();
        let mut owners = self.owner_tokens.lock().unwrap();
        let mut valid = self.valid_tokens.lock().unwrap();
        let mut due = Vec::new();
        while let Ok(action) = receiver.try_recv() {
            if valid.remove(&action.token) {
                let completed_owners = owners
                    .iter()
                    .filter_map(|(owner, token)| (*token == action.token).then_some(*owner))
                    .collect::<Vec<_>>();
                owners.retain(|_, token| *token != action.token);
                let mut targets = self.owner_targets.lock().unwrap();
                for owner in completed_owners {
                    targets.remove(&owner);
                }
                due.push(action);
            }
        }
        due
    }

    pub fn pending_target(&self, owner: QuantizedLaunchOwner) -> Option<PatternLaunchTarget> {
        self.owner_targets.lock().unwrap().get(&owner).cloned()
    }

    pub(crate) fn process_scheduler(
        &self,
        pending: &mut PendingQuantizedLaunches,
        rendered_beats: f64,
        frontier_beats: f64,
        playing: bool,
        song_active: bool,
    ) {
        pending.process(
            &self.request_rx.lock().unwrap(),
            &self.due_tx,
            rendered_beats,
            frontier_beats,
            playing,
            song_active,
        );
    }
}

fn canonicalize_target(
    target: PatternLaunchTarget,
    scene_count: usize,
    track_count: usize,
) -> Result<PatternLaunchTarget, QuantizedLaunchSubmitError> {
    let scene = match &target {
        PatternLaunchTarget::Scene { scene } | PatternLaunchTarget::SceneTracks { scene, .. } => {
            *scene
        }
    };
    if scene >= scene_count {
        return Err(QuantizedLaunchSubmitError::SceneOutOfRange { scene, scene_count });
    }
    match target {
        PatternLaunchTarget::Scene { .. } => Ok(target),
        PatternLaunchTarget::SceneTracks { scene, mut tracks } => {
            tracks.sort_unstable();
            tracks.dedup();
            if tracks.is_empty() {
                return Err(QuantizedLaunchSubmitError::EmptyTrackMask);
            }
            if let Some(track) = tracks.iter().copied().find(|track| *track >= track_count) {
                return Err(QuantizedLaunchSubmitError::TrackOutOfRange { track, track_count });
            }
            Ok(PatternLaunchTarget::SceneTracks { scene, tracks })
        }
    }
}

fn map_send_error<T>(error: TrySendError<T>) -> QuantizedLaunchSubmitError {
    match error {
        TrySendError::Full(_) => QuantizedLaunchSubmitError::RequestQueueFull,
        TrySendError::Disconnected(_) => QuantizedLaunchSubmitError::SchedulerDisconnected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(token: u64, quantize: LaunchQuantize, owner: u32) -> QuantizedLaunchMessage {
        QuantizedLaunchMessage::Schedule(QuantizedLaunchRequest {
            token,
            target: PatternLaunchTarget::Scene {
                scene: token as usize,
            },
            quantize,
            owner: QuantizedLaunchOwner::SceneMacro(owner),
            snapshot: None,
        })
    }

    #[test]
    fn strict_deadlines_use_sixteenth_and_named_bar_grid() {
        assert_eq!(launch_deadline(0.0, true, LaunchQuantize::Sixteenth), 0.25);
        assert_eq!(launch_deadline(0.25, true, LaunchQuantize::Sixteenth), 0.5);
        assert_eq!(launch_deadline(3.5, true, LaunchQuantize::Bar), 4.0);
        assert_eq!(launch_deadline(4.0, true, LaunchQuantize::Bar), 8.0);
        assert_eq!(launch_deadline(0.1, true, LaunchQuantize::Eighth), 0.5);
        assert_eq!(launch_deadline(0.1, true, LaunchQuantize::Quarter), 1.0);
        assert_eq!(launch_deadline(0.1, true, LaunchQuantize::Half), 2.0);
    }

    #[test]
    fn transport_labels_round_trip_every_supported_grid() {
        for quantize in [
            LaunchQuantize::Off,
            LaunchQuantize::Sixteenth,
            LaunchQuantize::Eighth,
            LaunchQuantize::Quarter,
            LaunchQuantize::Half,
            LaunchQuantize::Bar,
        ] {
            assert_eq!(
                LaunchQuantize::from_transport_label(quantize.transport_label()),
                Some(quantize)
            );
        }
    }

    #[test]
    fn lookahead_horizon_never_makes_a_launch_due_before_rendered_boundary() {
        let (tx, rx) = mpsc::sync_channel(1);
        let (due_tx, due_rx) = mpsc::sync_channel(1);
        let mut pending = PendingQuantizedLaunches::default();
        tx.try_send(request(1, LaunchQuantize::Sixteenth, 1))
            .unwrap();

        // A scheduler may already have rendered a lookahead horizon beyond
        // 0.25, but this pass deliberately accepts only rendered transport.
        pending.process(&rx, &due_tx, 0.249, 0.249, true, false);
        assert!(due_rx.try_recv().is_err());
        pending.process(&rx, &due_tx, 0.25, 0.25, true, false);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);
    }

    #[test]
    fn pending_pass_handles_immediate_boundary_cancellation_and_replacement() {
        let (tx, rx) = mpsc::sync_channel(8);
        let (due_tx, due_rx) = mpsc::sync_channel(8);
        let mut pending = PendingQuantizedLaunches::default();

        tx.try_send(request(1, LaunchQuantize::Off, 1)).unwrap();
        pending.process(&rx, &due_tx, 1.3, 1.3, true, false);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);

        tx.try_send(request(2, LaunchQuantize::Bar, 2)).unwrap();
        tx.try_send(QuantizedLaunchMessage::CancelToken(2)).unwrap();
        tx.try_send(request(3, LaunchQuantize::Bar, 3)).unwrap();
        tx.try_send(QuantizedLaunchMessage::CancelOwner(
            QuantizedLaunchOwner::SceneMacro(3),
        ))
        .unwrap();
        tx.try_send(request(4, LaunchQuantize::Sixteenth, 4))
            .unwrap();
        tx.try_send(request(5, LaunchQuantize::Sixteenth, 4))
            .unwrap();
        pending.process(&rx, &due_tx, 2.0, 2.0, true, false);
        assert!(due_rx.try_recv().is_err());
        pending.process(&rx, &due_tx, 2.25, 2.25, true, false);
        assert_eq!(due_rx.try_recv().unwrap().token, 5);
        assert!(due_rx.try_recv().is_err());
    }

    #[test]
    fn due_launches_carry_the_audible_deadline_not_the_request_or_drain_beat() {
        let (tx, rx) = mpsc::sync_channel(2);
        let (due_tx, due_rx) = mpsc::sync_channel(2);
        let mut pending = PendingQuantizedLaunches::default();

        // Requested mid-grid-interval: the stamped deadline is the exact next
        // bar boundary (4.0), and it survives even when the launch is drained
        // late (rendered 4.37).
        tx.try_send(request(1, LaunchQuantize::Bar, 1)).unwrap();
        pending.process(&rx, &due_tx, 2.6, 2.6, true, false);
        pending.process(&rx, &due_tx, 4.37, 4.37, true, false);
        let due = due_rx.try_recv().unwrap();
        assert_eq!(due.token, 1);
        assert_eq!(due.deadline_beats, 4.0);

        // Unquantized: stamped with the rendered beat at scheduling time.
        tx.try_send(request(2, LaunchQuantize::Off, 2)).unwrap();
        pending.process(&rx, &due_tx, 5.125, 5.125, true, false);
        let due = due_rx.try_recv().unwrap();
        assert_eq!(due.token, 2);
        assert_eq!(due.deadline_beats, 5.125);
    }

    #[test]
    fn stopped_transport_emits_all_quantization_modes_immediately() {
        for (idx, quantize) in [
            LaunchQuantize::Off,
            LaunchQuantize::Sixteenth,
            LaunchQuantize::Bar,
        ]
        .into_iter()
        .enumerate()
        {
            let (tx, rx) = mpsc::sync_channel(1);
            let (due_tx, due_rx) = mpsc::sync_channel(1);
            let mut pending = PendingQuantizedLaunches::default();
            tx.try_send(request(idx as u64 + 1, quantize, idx as u32))
                .unwrap();
            pending.process(&rx, &due_tx, 9.75, 9.75, false, false);
            assert_eq!(due_rx.try_recv().unwrap().token, idx as u64 + 1);
        }
    }

    #[test]
    fn full_due_channel_retries_exactly_once() {
        let (tx, rx) = mpsc::sync_channel(2);
        let (due_tx, due_rx) = mpsc::sync_channel(1);
        let mut pending = PendingQuantizedLaunches::default();
        due_tx
            .try_send(DuePatternLaunch {
                token: 99,
                target: PatternLaunchTarget::Scene { scene: 0 },
                deadline_beats: 0.0,
                scheduler_applied: false,
            })
            .unwrap();
        tx.try_send(request(1, LaunchQuantize::Off, 1)).unwrap();
        pending.process(&rx, &due_tx, 0.0, 0.0, true, false);
        assert_eq!(due_rx.try_recv().unwrap().token, 99);
        pending.process(&rx, &due_tx, 0.0, 0.0, true, false);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);
        pending.process(&rx, &due_tx, 0.0, 0.0, true, false);
        assert!(due_rx.try_recv().is_err());
    }

    #[test]
    fn target_masks_are_canonical_and_validated() {
        assert_eq!(
            canonicalize_target(
                PatternLaunchTarget::SceneTracks {
                    scene: 1,
                    tracks: vec![2, 0, 2]
                },
                2,
                3
            )
            .unwrap(),
            PatternLaunchTarget::SceneTracks {
                scene: 1,
                tracks: vec![0, 2]
            }
        );
        assert_eq!(
            canonicalize_target(
                PatternLaunchTarget::SceneTracks {
                    scene: 0,
                    tracks: vec![]
                },
                1,
                1
            ),
            Err(QuantizedLaunchSubmitError::EmptyTrackMask)
        );
    }

    #[test]
    fn command_registry_rejects_emitted_action_invalidated_in_transit() {
        let mailbox = QuantizedLaunchMailbox::default();
        let token = mailbox
            .schedule(
                PatternLaunchTarget::Scene { scene: 0 },
                LaunchQuantize::Off,
                QuantizedLaunchOwner::SceneMacro(7),
                1,
                1,
                None,
            )
            .unwrap();
        let mut pending = PendingQuantizedLaunches::default();
        mailbox.process_scheduler(&mut pending, 0.0, 0.0, true, false);
        mailbox.cancel_token(token).unwrap();
        assert!(mailbox.drain_valid_due().is_empty());
        assert_eq!(
            mailbox.pending_target(QuantizedLaunchOwner::SceneMacro(7)),
            None
        );
    }

    #[test]
    fn mailbox_reports_the_authoritative_pending_target_for_an_owner() {
        let mailbox = QuantizedLaunchMailbox::default();
        mailbox
            .schedule(
                PatternLaunchTarget::Scene { scene: 1 },
                LaunchQuantize::Bar,
                QuantizedLaunchOwner::Transport,
                3,
                1,
                None,
            )
            .unwrap();
        assert_eq!(
            mailbox.pending_target(QuantizedLaunchOwner::Transport),
            Some(PatternLaunchTarget::Scene { scene: 1 })
        );

        mailbox
            .schedule(
                PatternLaunchTarget::Scene { scene: 2 },
                LaunchQuantize::Bar,
                QuantizedLaunchOwner::Transport,
                3,
                1,
                None,
            )
            .unwrap();
        assert_eq!(
            mailbox.pending_target(QuantizedLaunchOwner::Transport),
            Some(PatternLaunchTarget::Scene { scene: 2 })
        );

        let mut pending = PendingQuantizedLaunches::default();
        mailbox.process_scheduler(&mut pending, 0.0, 0.0, false, false);
        assert_eq!(mailbox.drain_valid_due().len(), 1);
        assert_eq!(
            mailbox.pending_target(QuantizedLaunchOwner::Transport),
            None
        );
    }

    #[test]
    fn submitting_to_a_full_request_channel_fails_explicitly() {
        let mailbox = QuantizedLaunchMailbox::default();
        for owner in 0..REQUEST_CAPACITY {
            mailbox
                .schedule(
                    PatternLaunchTarget::Scene { scene: 0 },
                    LaunchQuantize::Bar,
                    QuantizedLaunchOwner::SceneMacro(owner as u32),
                    1,
                    1,
                    None,
                )
                .unwrap();
        }
        assert_eq!(
            mailbox.schedule(
                PatternLaunchTarget::Scene { scene: 0 },
                LaunchQuantize::Bar,
                QuantizedLaunchOwner::SceneMacro(999),
                1,
                1,
                None,
            ),
            Err(QuantizedLaunchSubmitError::RequestQueueFull)
        );
    }

    #[test]
    fn unrelated_epochs_do_not_affect_pending_launch_state() {
        let (tx, rx) = mpsc::sync_channel(2);
        let (due_tx, due_rx) = mpsc::sync_channel(2);
        let mut pending = PendingQuantizedLaunches::default();
        tx.try_send(request(1, LaunchQuantize::Bar, 1)).unwrap();
        pending.process(&rx, &due_tx, 1.0, 1.0, true, false);
        // A scheduler pattern/topology rebuild does not call any reset on this
        // state; processing resumes with the same pending deadline.
        pending.process(&rx, &due_tx, 4.0, 4.0, true, false);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);
    }
}
