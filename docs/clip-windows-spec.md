# Clip Off, Timed Clip Windows, and Graph-Triggered Clips — mini spec

Status: draft (rev 1, 2026-08-06). Grows out of the neural-groups work
(docs/neural-groups-spec.md): the idea that a graph node fire could play a
clip on a track for a while and then turn it off — "nested patterns." This
spec keeps to three layers, each independently useful, each building on the
last:

1. **Clip off** — a first-class live "this track plays nothing" gesture in
   session view (the mixer grid has launch but no stop).
2. **Timed windows** — "turn on clip X, play for N beats, revert."
3. **Graph native** — a node fire opens a window on its routed track instead
   of emitting a note, gated by a per-node param.

Two principles carried over from the discussion:

- **No quantization required.** Clips are phase-anchored to the global clock,
  so a mid-bar window-open joins the pattern in progress, in time. A window is
  a timed *unmute* over an always-running pattern, not a transport start.
  (Quantize remains available as an option — it's the existing launch
  machinery — but the graph path defaults to immediate.)
- **Ephemeral layers are never recorded.** User gestures (layer 1) are
  performance moves and SHOULD be captured like launches. Graph-driven
  windows (layer 3) are an ephemeral overlay like the graph delta store:
  invisible to takes/Capture/save-backs, cleared on transport stop and scene
  change, authored state always restorable underneath.

## 1. Layer 1 — live clip off

### 1.1 The gap

Session resolution is `effective_pattern_id(track) = track_overrides[track]
else scenes[current].cells[track]` (state/scenes.rs:774). The override is
`Option<PatternId>`, so "no pin" and "stopped" are the same value — there is
no way to *pin silence*. The audible gate already exists: `scene_silenced`
per lane (state/core.rs:13) is what the scheduler consults when deciding to
push triggers (scheduler/clock.rs:317), and the arrangement's explicit-empty
rows already land on that same flag (song_playback.rs:411). Session view just
has no writer for it as a deliberate gesture.

### 1.2 Design: adopt the arrangement's primitive

The arrangement solved this exact problem with
`ProjectSongTrackOverride.pattern_id: Option<u64>` where `None` means
**explicit empty** — "play nothing even though the scene cell has a pattern"
(song.rs:23-27). Session view adopts the same tri-state:

```rust
// state/scenes.rs
pub enum TrackPin {
    Pattern(PatternId),   // today's Some(pid)
    Silent,               // NEW: explicit clip-off pin
}
track_overrides: Vec<Option<TrackPin>>,   // None = no pin (scene cell rules)
```

`effective_pattern_id` maps `Silent → None` and the resolution site also sets
`scene_silenced` for the lane. Serde: `track_overrides` is runtime state, not
serialized — no version bump.

### 1.3 Semantics

- **Stop is a launch-shaped gesture.** New `PatternLaunchTarget::TrackStop
  { track }` through the same funnel (`apply_pattern_launch_at`,
  app/mod.rs:1568), so it gets quantize, pending-launch blink, owner-token
  replacement (`QuantizedLaunchOwner::TrackClip`), and chunk-split sample
  accuracy for free.
- **Capture records it.** `record_song_capture_launch` maps `TrackStop` to a
  capture event whose consolidation produces an explicit-empty override in
  the arrangement — the arrangement already represents this, so Capture of a
  live stop is lossless. Same for `observe_manual_clip_launch`'s sibling on
  the song-authority mixer path.
- **Save-back masking:** a `Silent` pin masks its lane exactly like a pattern
  pin does in `save_scene_snapshot_masked` (scenes.rs:548) — a stop must
  never cause the outgoing pattern's content to be cloned anywhere.
- **Cleared by** scene launch (`track_overrides.fill(None)` already does),
  and by launching any clip on that track (pin replacement).

### 1.4 UI

Mixer grid (ui/mixer.lisp:443-518): a stop affordance per track — a small
square button in the track's cell column header (Ableton's clip-stop
convention). Pressing it issues new host command `stop-track-clip`
`{:track :quantize}`. `TrackPatternCellView` (scenes.rs:885) gains a
`stopped` flag so the grid can render the stopped state (no cell `active`),
and `SEQ.queued-track-clips` blink covers the pending case unchanged.
Clicking the currently-active cell may later become stop-toggle; not in v1.

## 2. Layer 2 — timed windows

A window = an on-action now plus a scheduled off-action. No new timer
machinery: `PendingQuantizedLaunches` (quantized_launch.rs:190) already holds
beat-deadline actions with owner-token replacement and sample-exact
chunk-split installs.

```
open_window(track, pattern, dur_beats, quantize?):
    apply launch  TrackPattern { track, pattern }     (now, or at quantize boundary)
    schedule      TrackRevert  { track, generation }  at deadline = open_beat + dur_beats
```

- **`TrackRevert` reverts to the resolution underneath**, i.e. it clears the
  window's pin and lets `effective_pattern_id` fall back to the scene cell.
  If the cell holds a pattern, the track resumes it (interrupt-then-resume);
  if the cell is empty, the track goes silent (stab-over-silence). One rule
  covers both musical cases; there is no separate "revert to silence" mode —
  author the cell empty if you want silence after.
- **Generation counter per track**: a revert carries the generation of the
  open that scheduled it and is a no-op if a newer action (user launch, newer
  window, stop) has bumped it. This is the re-trigger policy: **re-trigger
  extends** (new window replaces old, old revert dies), user gestures always
  win.
- Owner token: reuse `QuantizedLaunchOwner::TrackClip(track)` so a window's
  revert and a user's pending launch on the same track replace each other
  coherently.
- Layer 2 exposed as host command `play-clip-window`
  `{:track :pattern-id :dur-beats :quantize}` — independently useful from
  lisp/processes before the graph native exists.

Windows opened *by the user* (host command) record their open as a normal
launch capture event; whether the scheduled revert also records (as a stop)
follows the same rule — user-owned windows are real gestures. Graph-owned
windows record nothing (§3.4).

## 3. Layer 3 — graph-triggered windows

### 3.1 Authoring surface

One per-node param toggle, per the discussion ("basically a toggle of whether
it targets the patterns or not"):

```lisp
:params ((clip-trigger :int 0 1 :default 0)   ; 0 = emit notes (today), 1 = open window
         ...)
```

When `clip-trigger >= 1`, a node fire does **not** emit a note; it opens a
window on the node's routed track for `dur = delay × dur-factor` beats (the
same duration expression the note path uses, so existing duration intuition
transfers). The clip is **whatever the track's scene cell holds** — the
window is an unmute, not a clip selector. (A per-node clip-index param is a
possible later extension; v1 keeps the palette in the scene where it already
lives.) Velocity is ignored in v1.

`clip-trigger` is a normal node param: p-lockable, group-assignable, visible
to all existing UI generation. All graph dynamics compose unchanged — loops,
cool-offs, vel-decay (unused but harmless), per-group polyphony, H coupling.
A fire that max-poly REJECTS opens nothing (arbitration stays upstream).

### 3.2 Emission plumbing

`GraphEmission.event` is note-shaped (`EmittedAccumulatorEvent`), so windows
do not ride it. `commit_firing` (runtime/graph.rs:2160) branches on the
param: instead of `push_emission_event`, push onto a new
`Vec<GraphWindowRequest> { sample_time, node_index, track, dur_beats }`
drained by the scheduler loop (lookahead.rs:1549-1645) alongside emissions.

The scheduler cannot mutate `ProjectScenes` (control-side mutex). Two-part
apply, mirroring the existing `DuePatternLaunch` protocol
(quantized_launch.rs:110):

- **Audible now (scheduler-local):** an ephemeral per-track window mask the
  trigger gate consults in addition to `scene_silenced`
  (scheduler/clock.rs:317) — this is what makes the window audible
  sample-exact without an epoch bump. Template: `MidiFxQuantizerState`
  (scheduler/midi_fx.rs:36-95) for the scheduler-local deadline list that
  closes it.
- **Mirror to control:** a mailbox message with `scheduler_applied = true`;
  the control thread pins/unpins the ephemeral layer for UI display and acks
  (`acknowledge_mirror`, no `pattern_epoch` bump — a bump would drop
  in-flight events).

### 3.3 Precedence

Graph windows are a layer BELOW user pins: `effective` = user pin, else graph
window, else scene cell. A user launch or stop on the track immediately
overrides and cancels the track's graph window state; the graph re-opens on
its next qualifying fire. This keeps the performer in charge without needing
to disable the graph.

### 3.4 The no-record rules (hard requirements)

Graph-driven windows must be invisible to every persistence/capture surface:

1. NOT through `apply_pattern_launch_at` — so `record_song_capture_launch`
   (song_capture.rs:276) never sees them. They use their own apply path.
2. NOT through `observe_manual_clip_launch` (song_capture.rs:324) and never
   latching (`latch_song_manual_override`).
3. Masked from `save_scene_snapshot_masked` like other non-self writes; a
   graph window must never cause cell content cloning.
4. Take recording (take_recording.rs:41-44 binds effective SoundRefs at
   punch-in): pending lanes resolve *through the authored layer*, ignoring
   graph-window pins, so a mid-take window can't rebind a lane's sound.
5. Cleared on transport stop, scene change, pattern change, and graph reset —
   same lifecycle as the graph delta store. The authored project after a
   session with graph windows is byte-identical to before.

## 4. Build order

- **P1 — clip off:** `TrackPin::Silent`, `TrackStop` target, capture-to-
  explicit-empty, mixer stop button. Independently shippable; fixes a real
  session-view gap regardless of everything else.
- **P2 — windows:** `TrackRevert` + generation counters + `play-clip-window`
  host command. Testable from lisp without graph changes.
- **P3 — graph native:** `clip-trigger` param, `GraphWindowRequest` drain,
  scheduler window mask + mirror, no-record shields, demo-script toggle
  column.

## 5. Open questions

1. Should a *user-owned* window's scheduled revert be captured as a stop
   event, or should capture consolidate the whole window into one
   clip-of-length-N in the arrangement? (Latter is more faithful to intent;
   needs consolidation support for timed overrides.)
2. Does a window-open retrigger the pattern's accum/pending state
   (`pending_accum_reset`) or join fully cold? Joining in progress suggests
   no reset; verify feel.
3. Per-node clip-index param (window as clip *selector*) — deferred; would
   need pool-slot addressing from graph params.
4. Should graph windows be visible in the mixer grid (e.g. a distinct pulse
   on the cell) in v1, or is the θΔ-style "trust the sound" approach fine
   until it isn't?
