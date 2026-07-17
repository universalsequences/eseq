# Quantized Scene-Launch Foundation

Status: prerequisite design for `MACRO_MAPPING_SPEC.md` Phase 6  
Scope: scheduler-owned timing and command-thread application of scene and
per-track pattern launches. This document does **not** implement scene macros.

## 1. Why this foundation is needed

Phase 6 scene macros need to steal target-scene patterns on press and return to
the captured origin on release, optionally at the next sixteenth-note or bar
boundary. The repository currently has immediate scene-launch operations, but
no facility that accepts a launch request, resolves its musical deadline in the
scheduler, and hands the due launch back to the command thread.

`ProcessRuntime::clear_scene_pending` is not that facility. It clears process
events, inlet writes, and conductor ticks; it neither represents nor emits a
scene launch. Phase 6 must depend on the mechanism specified here rather than
polling transport state from the UI/render loop or sleeping until a boundary.

## 2. Goals

- Schedule full-scene or selected-track pattern launches at `Off`, `Sixteenth`,
  or `Bar` quantization.
- Let the scheduler own musical-boundary calculation.
- Let the command thread remain the sole owner of project/graph mutation.
- Support cancellation and replacement without races or stale returns.
- Work without an open UI so the same path is usable by headless hosts/tests.
- Preserve the existing scene-switch restore path, including macro-effective
  parameter sends.

## 3. Non-goals

- Parameter morphing, scene-macro persistence, or scene-macro UI.
- Audio-thread mutation of project state.
- Sample-accurate pattern replacement inside an already-rendered audio block.
  The guarantee is that the due notification is emitted when the rendered
  transport reaches the requested musical boundary; ordinary command-thread
  application latency follows.
- Crossfading patterns or changing effect/instrument structure.

## 4. Ownership model

The mechanism has three explicit stages:

```text
command thread                 scheduler thread                 command thread
request launch  ───────────▶   resolve/track deadline  ─────▶   apply launch
cancel/replace  ───────────▶   discard stale request             restore graph/UI
```

The scheduler must never call `switch_pattern`, `launch_track_pattern`, rebuild
the graph, or touch UI state. It emits a small immutable due action. The command
thread drains due actions and invokes one shared application function containing
the existing project, graph, bus, default-restore, and publication work.

## 5. Public data model

Use stable request tokens so cancellation does not depend on queue position:

```rust
pub type QuantizedLaunchToken = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchQuantize {
    Off,
    Sixteenth,
    Bar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternLaunchTarget {
    Scene { scene: usize },
    SceneTracks { scene: usize, tracks: Vec<usize> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizedLaunchRequest {
    pub token: QuantizedLaunchToken,
    pub target: PatternLaunchTarget,
    pub quantize: LaunchQuantize,
    /// Identifies the logical owner, initially a scene-macro id. Requests from
    /// the same owner replace that owner's earlier pending request.
    pub owner: QuantizedLaunchOwner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuantizedLaunchOwner {
    SceneMacro(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuePatternLaunch {
    pub token: QuantizedLaunchToken,
    pub target: PatternLaunchTarget,
}
```

Tokens are monotonic and never reused during a process lifetime. Track lists are
canonicalized at submission: sorted, deduplicated, and validated against the
active track count. An empty track list is rejected rather than interpreted as
a full-scene launch.

## 6. Request and completion transport

Add two bounded, non-blocking channels owned by `SequencerState` (or a dedicated
`QuantizedLaunchMailbox` owned by it):

- command → scheduler: request/cancel messages;
- scheduler → command: `DuePatternLaunch` messages.

Submitting from the command thread must fail explicitly if the request channel
is full. The scheduler must not block. The due channel must be sized so a normal
command-thread stall cannot silently lose a launch; if it is full, retain the
due action in scheduler-local state and retry on the next scheduler iteration.
Never drop or coalesce due actions after their deadline unless they were
explicitly cancelled or superseded before becoming due.

The mailbox API should be independent of `metal_seq` so terminal/headless hosts
can drain and apply actions too.

## 7. Deadline semantics

The scheduler resolves deadlines from its authoritative transport clock, not
from UI playhead atomics.

- `Off`: emit as soon as the scheduler receives the request.
- `Sixteenth`: next strict `0.25` quarter-note boundary.
- `Bar`: next strict bar boundary using the project's current meter. Until the
  sequencer has variable meter, define a bar as four quarter notes in one named
  helper; do not scatter the constant.
- A request submitted exactly on a boundary targets that boundary only if it
  can still be applied before scheduling has crossed it; otherwise it targets
  the next boundary. Put this decision in a tested helper using a small epsilon.
- While transport is stopped, all quantization modes emit immediately. Waiting
  for a transport that is not advancing would create permanently pending UI
  state.

Store deadlines in musical beats and compare them against the rendered
transport position, not the lookahead horizon. Emitting from the lookahead
horizon would switch project state early by the scheduler's lookahead depth.

## 8. Cancellation and replacement

Messages are:

```rust
enum QuantizedLaunchMessage {
    Schedule(QuantizedLaunchRequest),
    CancelToken(QuantizedLaunchToken),
    CancelOwner(QuantizedLaunchOwner),
    CancelAll,
}
```

Rules:

- A new request for an owner supersedes that owner's pending request.
- Cancellation is effective only before the scheduler has emitted the due
  action. After emission, the command thread uses a token-validity registry to
  reject actions invalidated while in transit.
- Manual scene switching cancels all pending scene-macro launches and invalidates
  any undrained scene-macro tokens before applying the manual switch.
- Transport stop does not discard requests; because stopped requests are due
  immediately, they are emitted and applied. Project load/reset uses
  `CancelAll` and clears the command-side validity registry.
- Scene deletion/reordering invalidates pending requests whose target identity
  can no longer be resolved. Phase 6 may initially cancel all pending scene
  macro requests for either operation.

## 9. Command-thread application seam

Extract the duplicated scene-switch body into one host-level operation, e.g.:

```rust
fn apply_pattern_launch(&mut self, target: &PatternLaunchTarget)
    -> Result<PatternLaunchOutcome, PatternLaunchError>;
```

For a full scene it must perform the same work as the current
`"switch-pattern"` path: bus snapshot switch, `SequencerState::switch_pattern`,
sample-id application, instrument run-mode synchronization, mod-route sync,
and `push_all_restored_defaults`.

For `SceneTracks`, resolve the target scene's pattern cell for each selected
track and use the existing per-track launch operation. Apply all selected tracks
as one logical transaction, then publish/synchronize once. Validation happens
before mutation so a bad scene or track cannot leave a partially launched set.

UI reactive refresh is a consumer of `PatternLaunchOutcome`, not part of the
core mutation. Both direct host commands and due scheduler actions must call the
same core application seam.

## 10. Scheduler integration

Keep pending launch state beside `SchedulerLookaheadState`, not inside
`ProcessRuntime`; pattern launches are host transport actions, not process
events. At each scheduler iteration:

1. Drain request/cancel messages.
2. Resolve new deadlines using the current clock and playing state.
3. Compare deadlines with rendered transport beats.
4. Move due actions to the completion channel, retaining any action that cannot
   yet be sent.

Pattern/topology epoch changes must not accidentally clear these requests.
Only the cancellation rules in §8 do. This prevents unrelated edits from
silently eating a queued launch.

## 11. Phase 6 usage contract

With this foundation, a scene macro owns at most one pending launch token:

- engage with `steal_patterns`: capture origin, schedule target;
- release before target becomes due: cancel target and schedule nothing;
- release after target applies: schedule captured origin;
- repeated press/release: owner replacement prevents stale launches;
- manual scene switch: cancel owner(s), release morph overrides, invalidate
  returns, then apply the manual switch;
- overlapping scene macros: last scheduled press wins pattern state; each macro
  retains its own captured origin, matching the Phase 6 policy.

Whether a target launch actually applied must be observable by Phase 6 so it
does not schedule a return for a target that was cancelled before its deadline.

## 12. Deterministic tests

Add a scheduler harness that drives request input, rendered sample position, BPM,
and transport state without launching audio or UI. Required tests:

1. `Off` emits immediately.
2. Sixteenth and bar requests emit at the exact next rendered boundary, never
   at the lookahead boundary.
3. A stopped transport emits immediately for every quantization setting.
4. Cancelling by token and owner prevents emission.
5. Replacing an owner's request emits only the replacement.
6. A full due channel retains and later emits the action exactly once.
7. Pattern/topology epoch changes do not clear pending launches.
8. Manual-switch invalidation rejects an already-emitted but undrained action.
9. Masked launches validate all tracks before mutation and update only the mask.
10. The shared full-scene application seam performs restored effective sends.

The scheduler tests should exercise the extracted production pending-launch pass
directly, following the existing deterministic lookahead-harness pattern in
`AGENTS.md`.

## 13. Delivery order

1. Extract and test the shared command-thread application seam.
2. Add mailbox types, validation, and token registry.
3. Add the pure deadline/pending-launch scheduler state and deterministic tests.
4. Wire the scheduler thread and every active host loop to drain due actions.
5. Route existing immediate scene/per-track commands through the shared seam.
6. Only then implement `MACRO_MAPPING_SPEC.md` Phase 6 pattern steal/return.

## 14. Acceptance criteria

This foundation is complete when scene and masked-track launches can be
submitted with all three quantization settings; deterministic tests prove
boundary, cancellation, replacement, and backpressure behavior; all host
surfaces use the same application seam; and no timing decision depends on UI
frame cadence, sleeps, or scheduler lookahead distance.
