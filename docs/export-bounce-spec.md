# Arrangement bounce specification

Status: implementation contract for **eseq-45bn**. A standalone saved-project
export command is available as of 2026-09-08; see [usage](export-bounce.md).
A command-driven modal wraps saved-project export. Live-source capture is a future
integration gate; disconnected snapshot/rebinding scaffolding has been removed.
Timing contract revised 2026-09-07: preserve playback timing, with sample-accurate
notes/gates and block-boundary ordinary DSP updates.
Related: [song mode](song-mode-spec.md) and
[takes and additive arrangement recording](takes-and-additive-arrangement-recording-spec.md).

## 1. Decision and scope

**Implement an offline, sample-clock-driven stereo master bounce.** Run the
production song scheduler and audio graph in an isolated render session, as fast
as they can complete. Do not implement export by scripting Play / Wav / Stop,
and do not silently fall back to realtime capture for unsupported projects.

The existing **Wav live-record button stays unchanged**, including its recording
workflow. It remains the way to capture an improvised/live session or an exact
unrepeatable performance. Export renders the arrangement, not the user's current
live overrides, held keys, pending launches, or already-running DSP state.

V1 includes the entire arrangement or an explicit beat range, the audible master
mix (track/rack/bus processing, sends, routing, gains, authored mute/solo controls),
and a fixed user-selected tail. No stems, normalization, MP3, external-device
round trips, or automatic silence-based tail detection in this version.

Why not automated realtime capture: it still needs exact sample boundaries,
state isolation, reliable writing, end-of-song release semantics, and error
handling. It also inherits callback dropouts and the scheduler/UI races below.
That is not a cheap correctness-preserving substitute for an offline driver.
Offline rendering is an architectural change, not merely calling the callback in
a tight loop; if its prerequisites are unfinished, export remains unavailable.

## 2. Existing seams and implementation requirements

Paths below are relative to the repository root.

| Existing code | Consequence for bounce |
| --- | --- |
| `crates/sequencer/src/audio/callback.rs`: dispatches block events, calls `render_chunk`, then `master_recorder.capture`, then preview/metronome mixing | Reuse the musical render path and capture at the same pre-preview/pre-metronome seam. Do not include those monitoring sounds or hardware output latency. |
| `crates/sequencer/src/audio/render.rs`: `render_chunk` | Reuse graph/DSP rendering; an instrument-only probe is not a song renderer. |
| `crates/sequencer/src/scheduler/lookahead.rs`: `schedule_playing_lookahead` | Reuse the production lookahead pass, including graph/neural propagation, processes and MIDI FX; do not write a second note scheduler. |
| `crates/sequencer/src/scheduler/worker.rs`: rendered-sample frontier, sleeps, wall-clock roll-start hold | Extract driver-independent advancement. Offline scheduling must finish the required horizon before rendering, without sleeps or racing a worker. Disable live roll/input handling. |
| `crates/sequencer/src/ui/event_loop.rs`: `drain_due_mixer_controls` | Musical mute/solo changes currently drained by UI frames must become block-boundary render-session commands shared by playback and bounce. Running a hidden UI loop is not a solution. |
| `crates/sequencer/src/sequencer/state/song_runtime.rs`: `SongPlaybackRuntime`, `RuntimeSong::end_beat` | Use the preflighted arrangement and its row/clip boundaries; force a finite, non-looping pass, not open-ended capture. |
| `crates/sequencer/src/recorder.rs`: `MasterRecorder`, `save_recording_wav` | Current capture grows an in-memory vector under a try-lock and counts dropped blocks; the float writer clamps to unity. Neither is the export sink contract. Leave Wav behavior alone and implement bounded streaming export. |
| `crates/sequencer/src/app/graph/latency.rs`: `LatencyPlan` | Use the installed graph's compensation, not an estimated device/block delay. See section 4. |

The render session owns a frozen project snapshot, resolved assets and compiled
code, fresh scheduler/VM/generator/process state, graph, event queues, and an
integer rendered-frame cursor. Materialize the same production song state used
by playback, without editing the active project or sharing its mutable DSP/VM
instances. Compile/load failures, missing assets and generator errors fail
preflight or the job; never export a silently incomplete mix.

Sampler files are reopened during export preparation, using playback's WAV
decoder, channel conversion, and leading-silence trimming. Their contents may
differ from the buffers already loaded in the live project; this is accepted.
Resolve the captured sample references against the originating project's paths,
then load each distinct file once into the worker's graph. Those prepared
buffers remain fixed throughout the render, including across row changes.
Missing or unreadable referenced files fail preparation; an authored blank
sampler remains blank. Exact preservation of live sampler PCM is not required.
This exception does not change compiled instrument/effect asset verification.

For each fixed-size graph block: apply due prepared row/mixer updates, resolve
song transitions and schedule the complete required horizon, dispatch note/gate
events at their frame offsets, render the same graph, consume the requested
master frames, and advance the cursor. Graph publications must be acknowledged
before their designated block renders. Queue capacity is bounded: chunk work or
fail explicitly, never discard events. UI progress/cancel uses messages only.

### Timing contract shared with playback

- Notes, gate releases and other existing sample-accurate voice events retain
  their frame offsets. Song note scheduling still switches row snapshots at
  the exact musical boundary; parameter timing does not move those notes.
- Ordinary DGen/effect parameters use the existing block-boundary application
  path, including its ordering/coalescing of parameter locks within a block.
  Bounce does not require a new sample-accurate DGen parameter system.
- Row graph state and sequenced mixer holds must be applied independently of
  UI frames. A row/control edge at source frame f takes effect at the first
  graph-block start at or after f: Q(f) = ceil(f / K) * K, where K is the
  production graph block size and the session origin is frame zero. At one
  boundary, consume edges in source-time order, with releases before engages
  at equal source times; the final ordinary parameter value governs that block.
  A hold wholly between block starts can therefore coalesce to no audible hold.
- Prepare required assets, bindings and graph publications off the audio thread.
  The UI mirrors applied musical state; its redraw cadence is not a clock.
- Keep the production graph block size for every render, including the last
  block. Trim only the exported sample interval. Do not split spectral kernels
  at arbitrary row, parameter, range or file-end boundaries: existing generated
  convolution depends on hop-compatible blocks. Fixed-block processing is an
  intentional supported contract, not a prerequisite compiler defect to fix.

Parity checks compare these timing classes using the same block size and origin.
Sub-block parameter interpolation, sample-accurate ordinary effect automation,
and a general DGen parameter-event ABI are outside v1 scope.

## 3. Range, initial state and transport end

Default range is `[0, RuntimeSong::end_beat)`. An explicit selection is `[A, B)`
with finite `0 <= A < B <= end_beat`. Reject empty arrangements and invalid
ranges. Loop settings do not repeat the export. Use the snapshot's authored
transport/tempo semantics, not the current playhead or wall clock.

Use one shared beat-to-frame boundary convention with playback: a boundary falls
on the first sample at or after its beat. At constant tempo this is
`F(b) = ceil(b * 60 * sample_rate / BPM)`; avoid independently rounding each row's
duration. If playback's current crossing arithmetic differs, unify and test it
before claiming parity. Let `S = F(A)` and `E = F(B)`.

Always initialize at arrangement beat zero. For a selection, render the entire
prefix and discard it rather than seeking to A or resetting generators there.
This preserves held voices, reverb history, accumulators, process state and random
draw order. Preparation progress includes this prefix. No arbitrary warm-up
seconds or reuse of the interactive engine's history.

At source frame E, stop admitting note-ons, generator ticks/propagation and new
arrangement launches. Release all active gates (including sustain-held notes);
cancel future retriggers and note-ons, but preserve release semantics. Do not
invoke a generic transport stop that clears voices or resets effect buffers.
Continue DSP and time-dependent effect modulation through the tail at the final
tempo; hold the final authored parameter/mixer values. This is a defined
end-of-range release, not playback of the next arrangement section. A note-on
exactly at B is excluded. Ordinary row/control updates whose effective block
boundary is at or after E are not applied during the tail. An already sounding
one-shot may decay into the tail. The graph still renders complete blocks; gate
release at E uses its existing sample-offset event mechanism.

## 4. PDC and tails: the exact file interval

PDC aligns parallel paths; it is **not** reverb decay time. Let L be the installed
master path latency in frames, including any terminal DSP latency. The existing
plan exposes `mix_latency`; if later master processing adds latency it must be
included explicitly. Never add CPAL/device presentation latency to L.

Freeze topology and latency for a job. A project requiring a latency-changing
configuration during the range must fail preflight until a tested time-varying
latency design exists; ordinary parameter automation that leaves latency fixed
is allowed. Do not trim each track independently or disable compensation.

Current limitation: `RACK_SLOT_JOIN_UNCOMPENSATED` documents that computed
`rack_slot_pads` are not installed. Fix the rack join in the shared playback graph
before accepting exports requiring nonzero rack-slot pads. Preflight must reject
those projects while the gap exists, not describe the result as compensated.

Tail is a duration in seconds, **default 10 s**, adjustable from **0 to 600 s**.
Convert once to `T = ceil(tail_seconds * sample_rate)`. Render from zero through
`E + L + T` (exclusive); write exactly the master interval
`[S + L, E + L + T)`. The file has `E - S + T` stereo frames. This removes initial
algorithmic latency while retaining its flush at the end. For example, a
48,000-frame range, L=256 and T=480,000 produces 528,000 frames, not 528,256.

T=0 is an explicit hard cut after the compensated range. Positive T includes
instrument release, delayed repeats, convolution and reverberation together;
there is no additional hidden voice-tail allowance. Do not auto-trim silence or
apply a hidden fade. Show a truncation warning if the final 100 ms exceeds
-80 dBFS peak on either channel; explain that this is a heuristic, not proof of
silence (a long delay can produce a later echo). The user can increase T and
re-export. Infinite feedback/freeze is bounded by T; no energy detector may make
an export hang. Reject non-finite audio with the failing frame identified.

## 5. Determinism and playback equivalence

**V1 does not promise that bouncing a generative arrangement reproduces a prior
interactive pass, nor that repeated bounces are bit-identical.** Display this
limitation in the export dialog, not only here. A graph “seed event” is a musical
input, not evidence of a reproducible random seed. The native neural runtime in
`crates/sequencer/src/neural.rs` has random state seeded from network identity;
that alone proves neither all reset paths nor other runtimes deterministic.

The required equivalence is the shared musical execution semantics from a fresh
beat-zero start: same song transitions, event routing, p-locks, controls, end
release and PDC for the same inputs/state. For a fixed deterministic fixture,
playback and offline drivers must produce identical note/gate frame traces and
ordinary DSP/control block-boundary traces
and audio within a declared numerical tolerance. CPU architecture, compiler,
SIMD and parallel reduction order preclude a blanket cross-platform bit guarantee.

Graph/neural, probabilistic steps, MIDI FX and processes run through their normal
production runtimes, not baked substitutes. Prefix rendering preserves their
history within that pass. Random/entropy-dependent Lisp, mutable external data,
DSP noise initialization and runtime reset behavior can make a fresh bounce
sound different. Do not add a cosmetic “seed” setting while those sources remain
uncontrolled. Live MIDI/audio input or external hardware dependencies are not
replayable offline: reject them with an actionable explanation, never silently
replace them with silence. Scripts requiring real-time I/O must also fail rather
than hang the worker; preflight/cancellation capability belongs to session design.

A later reproducible mode requires persisted seed/state ownership for every RNG
(including Lisp and DSP), stable event ordering, reset rules shared with playback,
versioned code/assets, and enforced clock/I/O capabilities. Merely replacing
`Instant` or seeding one network is insufficient. Until then, preserve an exact
live generative performance with Wav; export is a new arrangement performance.

## 6. UI and file contract

User-revised entry (2026-09-08): **M-x export-song** opens a Lisp modal.
Do not add a toolbar button or dropdown menu; a future actions menu is a separate
issue. The current modal wraps saved-project export and identifies the source
explicitly. It offers an editable filename, entire arrangement or explicit beat
bounds (zero-based), sample rate, and tail seconds. Output is stereo **WAV,
32-bit IEEE float**, defaulting to 48 kHz and a 10-second tail. Each alternate
rate initializes the isolated graph at that rate without changing the live device.
Integer formats and dither remain deferred.

Suggest `<project-name> (1).wav`, incrementing to an available name in the app's
recordings directory. Reject path separators in typed names. The modal rejects
existing destinations; the standalone command retains its explicit `--replace`
option. Completion offers Finder reveal on macOS or folder open on Linux.
Validate RIFF size before rendering; RF64 is not silently substituted.
Float output preserves finite over-unity samples without normalization or clamp.

Stream to a uniquely created sibling temporary file with bounded memory. Offline
writing may backpressure rendering. Finalize the header, close successfully, then
atomically publish without overwriting an unconfirmed destination (including a
file created while the job ran). Disk-full, writer/graph/generator failures and
cancellation remove the temporary file and preserve any existing destination.
Success means a finalized file, never a partial WAV or dropped blocks.

The saved-project modal copies saved arrangement data into its private job and
runs the same executable in worker mode before live engine initialization. It
never stops live transport or recording and does not mutate project/history.
Only one export job runs at a time; reopening the command shows its progress.
Closing the modal does not cancel the job; cancellation is a separate action.
Progress, completion, cancellation and errors travel as atomically replaced
structured status documents, independently of human-readable worker logs.

The saved-project worker constructs a `PreparedExport` only after loading, sample
analysis, song preflight and latency preparation succeed. It owns the App and
headless engine, keeping their teardown order explicit. The UI and CLI share
project/range validation. Export errors distinguish validation, preparation,
rendering, writing and publication; worker status carries the stage and diagnostic.
Project loading still uses App and its existing loader; this is not an independent
project preparation library.

Capturing the current unsaved project with retained draft DSP sources remains a
separate integration gate. Do not label the saved-project path as live capture.

## 7. Implementation gates and checks

These are acceptance contracts for the implementation beads, not claims that
this documentation change has added tests. Use headless, Linux-runnable fixtures
and exact nextest selections, not whole-project/UI playback as the primary oracle.

- Shared render-session driver: deterministic song with row changes, takes,
  sends, MIDI FX, graph/neural seed propagation, processes and block-boundary
  mixer controls. Compare exact note/gate offsets and ordinary DSP/control block
  boundaries between offline and production drivers;
  stress event capacity and graph publication. No audio device or UI frame drain.
- Fixed blocks: preserve convolution output across non-block-aligned row changes,
  ranges and file ends; assert every DSP call receives K frames. Check ordered
  control coalescing, short holds and UI-independent row application.
- Boundaries: beat-zero impulse, last admitted event, event exactly at B,
  non-block-aligned E, fractional beat lengths, loops disabled, empty/invalid
  ranges. Assert exact frame counts, prefix-equivalent selection and gate release.
- PDC: dry/delayed parallel paths and bus sends; compensated impulse position,
  zero/positive tail, L flushed exactly once. Rack-pad-required projects reject
  until the shared graph fix has its own alignment test.
- Tails: long gate, one-shot, delay gap, reverb, feedback/freeze. Verify no new
  generated notes after E, retained DSP history, bounded duration and truncation
  warning behavior. No silence heuristic changes the specified frame count.
- Determinism: repeat deterministic fixtures with fixed inputs; compare event
  traces exactly and state a justified audio tolerance. Exercise generative
  runtimes without asserting an unsupported repeatability guarantee. Verify the
  warning and explicit rejection of non-replayable external dependencies.
- Writer/lifecycle: decode WAV to verify rate/channels/float depth and over-unity
  preservation; simulate disk failure, cancellation, destination races and RIFF
  limits. Verify no partial published file, bounded memory and unchanged project.
- UI integration: validate dialog bindings, finite/nonzero control geometry,
  disabled states, progress/error/cancel and unchanged Wav actions. Capture the
  actual arrangement panel via `metal_seq capture` on macOS when UI is built;
  keep that platform check separate from the headless engine gates.
