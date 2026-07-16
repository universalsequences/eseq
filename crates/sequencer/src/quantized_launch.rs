use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizedLaunchRequest {
    pub token: QuantizedLaunchToken,
    pub target: PatternLaunchTarget,
    pub quantize: LaunchQuantize,
    pub owner: QuantizedLaunchOwner,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuePatternLaunch {
    pub token: QuantizedLaunchToken,
    pub target: PatternLaunchTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuantizedLaunchMessage {
    Schedule(QuantizedLaunchRequest),
    CancelToken(QuantizedLaunchToken),
    CancelOwner(QuantizedLaunchOwner),
    CancelAll,
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

#[derive(Default)]
pub(crate) struct PendingQuantizedLaunches {
    pending: HashMap<QuantizedLaunchToken, PendingLaunch>,
    owner_tokens: HashMap<QuantizedLaunchOwner, QuantizedLaunchToken>,
    due_backlog: Vec<PendingDueLaunch>,
}

impl PendingQuantizedLaunches {
    pub(crate) fn process(
        &mut self,
        request_rx: &Receiver<QuantizedLaunchMessage>,
        due_tx: &SyncSender<DuePatternLaunch>,
        rendered_beats: f64,
        playing: bool,
    ) {
        loop {
            match request_rx.try_recv() {
                Ok(message) => self.handle_message(message, rendered_beats, playing),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
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
        playing: bool,
    ) {
        match message {
            QuantizedLaunchMessage::Schedule(request) => {
                if let Some(replaced) = self.owner_tokens.insert(request.owner, request.token) {
                    self.pending.remove(&replaced);
                }
                let deadline_beats = launch_deadline(rendered_beats, playing, request.quantize);
                self.pending.insert(
                    request.token,
                    PendingLaunch {
                        request,
                        deadline_beats,
                    },
                );
            }
            QuantizedLaunchMessage::CancelToken(token) => self.cancel_token(token),
            QuantizedLaunchMessage::CancelOwner(owner) => {
                if let Some(token) = self.owner_tokens.remove(&owner) {
                    self.pending.remove(&token);
                }
                self.due_backlog.retain(|due| due.owner != owner);
            }
            QuantizedLaunchMessage::CancelAll => {
                self.pending.clear();
                self.owner_tokens.clear();
                self.due_backlog.clear();
            }
        }
    }

    fn cancel_token(&mut self, token: QuantizedLaunchToken) {
        if let Some(pending) = self.pending.remove(&token) {
            if self.owner_tokens.get(&pending.request.owner) == Some(&token) {
                self.owner_tokens.remove(&pending.request.owner);
            }
        }
        self.due_backlog.retain(|due| due.action.token != token);
    }
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
    ) -> Result<QuantizedLaunchToken, QuantizedLaunchSubmitError> {
        let target = canonicalize_target(target, scene_count, track_count)?;
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let request = QuantizedLaunchRequest {
            token,
            target: target.clone(),
            quantize,
            owner,
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
        playing: bool,
    ) {
        pending.process(
            &self.request_rx.lock().unwrap(),
            &self.due_tx,
            rendered_beats,
            playing,
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
        pending.process(&rx, &due_tx, 0.249, true);
        assert!(due_rx.try_recv().is_err());
        pending.process(&rx, &due_tx, 0.25, true);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);
    }

    #[test]
    fn pending_pass_handles_immediate_boundary_cancellation_and_replacement() {
        let (tx, rx) = mpsc::sync_channel(8);
        let (due_tx, due_rx) = mpsc::sync_channel(8);
        let mut pending = PendingQuantizedLaunches::default();

        tx.try_send(request(1, LaunchQuantize::Off, 1)).unwrap();
        pending.process(&rx, &due_tx, 1.3, true);
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
        pending.process(&rx, &due_tx, 2.0, true);
        assert!(due_rx.try_recv().is_err());
        pending.process(&rx, &due_tx, 2.25, true);
        assert_eq!(due_rx.try_recv().unwrap().token, 5);
        assert!(due_rx.try_recv().is_err());
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
            pending.process(&rx, &due_tx, 9.75, false);
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
            })
            .unwrap();
        tx.try_send(request(1, LaunchQuantize::Off, 1)).unwrap();
        pending.process(&rx, &due_tx, 0.0, true);
        assert_eq!(due_rx.try_recv().unwrap().token, 99);
        pending.process(&rx, &due_tx, 0.0, true);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);
        pending.process(&rx, &due_tx, 0.0, true);
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
            )
            .unwrap();
        let mut pending = PendingQuantizedLaunches::default();
        mailbox.process_scheduler(&mut pending, 0.0, true);
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
            )
            .unwrap();
        assert_eq!(
            mailbox.pending_target(QuantizedLaunchOwner::Transport),
            Some(PatternLaunchTarget::Scene { scene: 2 })
        );

        let mut pending = PendingQuantizedLaunches::default();
        mailbox.process_scheduler(&mut pending, 0.0, false);
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
        pending.process(&rx, &due_tx, 1.0, true);
        // A scheduler pattern/topology rebuild does not call any reset on this
        // state; processing resumes with the same pending deadline.
        pending.process(&rx, &due_tx, 4.0, true);
        assert_eq!(due_rx.try_recv().unwrap().token, 1);
    }
}
