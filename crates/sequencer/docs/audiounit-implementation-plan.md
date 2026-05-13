# AudioUnit Backend Implementation Plan

## Goal

Add a macOS-only AudioUnit/CoreAudio backend that keeps the app "pure Rust", preserves the `cargo run` workflow, and enables proper audio-thread workgroup integration for the existing multi-worker audiograph engine.

This is meant to replace the current `cpal` output path on macOS, or at minimum provide a selectable macOS backend that can become the default once stable.

## Why

The current likely failure mode is not the DSP kernels themselves. It is callback deadline misses caused by helper workers that are not truly co-scheduled with the hardware IO thread.

That produces:

- late completion of `process_next_block()`
- device underruns / callback discontinuities
- audible clicks
- delays and reverbs turning those clicks into obvious "impulses from silence"

Mach RT promotion helps individual threads, but it does not fully solve the problem for a parallel render graph. The missing piece is joining helper workers to the actual audio workgroup associated with the device render thread.

## Constraints

- Keep the app pure Rust at the application layer
- Preserve `cargo run`
- Keep the existing Rust/C DSP engine
- Avoid Swift
- Preserve non-macOS portability by keeping the current backend as fallback
- Keep RT paths allocation-free and lock-free

## High-Level Approach

1. Introduce a macOS-specific audio backend in Rust.
2. Use AudioUnit/CoreAudio FFI directly from Rust.
3. Run the existing audiograph from the AudioUnit render callback.
4. Obtain the render thread's audio workgroup token from the macOS audio stack.
5. Pass that token into the existing C engine via `engine_set_os_workgroup()`.
6. Allow audiograph helper threads to join that workgroup.
7. Keep `cpal` as fallback while the backend is being proven out.

## Proposed Backend Shape

### New files

- `src/audio_backend.rs`
  - backend trait / common interface
- `src/audio_cpal.rs`
  - current implementation moved out of `src/audio.rs`
- `src/audio_macos.rs`
  - macOS AudioUnit backend
- `src/audio.rs`
  - thin dispatch layer that selects backend by platform / config

### Optional FFI files

- `src/macos/audio_unit.rs`
  - low-level CoreAudio / AudioUnit FFI
- `src/macos/workgroup.rs`
  - workgroup token handling and bridging

If the Rust crate ecosystem is good enough, use sys crates. Otherwise define the minimal FFI surface locally.

## Runtime Model

### Current

- `cpal` callback invokes `audio_callback()`
- `audio_callback()` pushes params/events and calls `process_next_block()`
- audiograph workers are started independently
- workgroup plumbing exists in C, but the host does not provide the real audio workgroup

### Target

- AudioUnit render callback invokes the same Rust-side `audio_callback()`
- callback is the actual hardware IO thread
- backend extracts the real workgroup associated with that callback context
- backend calls `engine_set_os_workgroup()`
- audiograph workers join the same workgroup
- parallel render becomes deadline-coupled to the real device callback

## Implementation Phases

## Phase 1: Stabilize Current Engine Before Backend Work

Do this first so debugging remains clear.

- Change default worker count from hardcoded `6` to a conservative policy
  - default to `0` or `1` workers unless workgroup integration is active
- Add instrumentation:
  - peak callback duration
  - overrun counter
  - last overrun timestamp
  - unsmoothed instantaneous CPU/load metric
- Keep the smoothed CPU meter, but also expose worst-case timing

This gives a baseline and a fallback even before AudioUnit is finished.

## Phase 2: Refactor Audio Backend Boundary

Create a backend abstraction so `audio_callback()` logic does not depend on `cpal`.

Suggested shape:

- prepare shared callback state
- backend owns device/stream lifecycle
- backend invokes a shared render function with `&mut [f32]`

The important outcome is:

- `audio_callback()` remains engine-focused
- transport/sequencer behavior stays unchanged
- only the host output path changes

## Phase 3: Build Minimal macOS AudioUnit Output

Implement a minimal output unit backend that can:

- create the output AudioUnit
- configure sample rate, format, channel count, and buffer size
- register a render callback
- start / stop cleanly

Requirements:

- non-interleaved vs interleaved format choice must be explicit
- callback must write directly into AudioUnit-provided buffers or a small stack/owned scratch buffer
- no allocations in callback
- no locks in callback

At this stage, it is acceptable to run single-threaded audiograph processing first.

Success criteria:

- `cargo run` on macOS plays audio through AudioUnit
- callback timing is stable
- feature parity with current output path for basic playback

## Phase 4: Workgroup Integration

Once the AudioUnit callback is active, add real workgroup integration.

Tasks:

- obtain the audio workgroup token from the render thread / audio unit context
- retain it safely on the host side as required by API semantics
- pass it into `engine_set_os_workgroup()`
- ensure worker rejoin / leave behavior is correct on start, stop, device change, and teardown

Validate:

- workers actually join the workgroup
- logging confirms join/leave behavior
- no use-after-free or stale token behavior

Important:

- workgroup setup should happen outside the render callback if possible, or at least only once
- failure to obtain/join workgroup must fail safe
  - either keep workers at `0/1`
  - or fall back to single-threaded mode

## Phase 5: Multithreaded Policy

Once workgroup joining is real:

- re-enable multi-worker execution on macOS
- derive default worker count from graph size and hardware
- do not use a fixed default like `6`

Suggested policy:

- tiny graph: `0` workers
- moderate graph: `1-2`
- large graph: scale up gradually

The key is to avoid paying worker wakeup/join overhead for trivial projects.

## Phase 6: Device and Lifecycle Hardening

Handle:

- output device changes
- sample rate changes
- channel count changes
- stop/start transitions
- app shutdown
- backend restart after failure

Rules:

- clear workgroup on shutdown
- stop workers before destroying graph
- avoid releasing workgroup objects before workers have left

## API/Code Changes

## Rust-side changes

### `src/audio.rs`

- split current `cpal` implementation out
- expose a backend-neutral render entry point

### `src/main.rs`

- select backend by platform
- allow env override, for example:
  - `TINYSEQ_AUDIO_BACKEND=cpal`
  - `TINYSEQ_AUDIO_BACKEND=audiounit`

### config/env

Add:

- `TINYSEQ_AUDIO_BACKEND`
- `TINYSEQ_AUDIOGRAPH_WORKERS`
- `TINYSEQ_AUDIOGRAPH_MACH_RT`
- `TINYSEQ_AUDIOGRAPH_RT_LOG`

Consider adding:

- `TINYSEQ_AUDIOGRAPH_FORCE_SINGLE_THREADED=1`

## C engine changes

Minimal if possible. The existing hooks are already promising:

- `engine_set_os_workgroup()`
- `engine_clear_os_workgroup()`
- worker join/leave versioning

Possible additions:

- explicit "workgroup active" query
- overrun / scheduling diagnostics exported to Rust
- optional policy hook for worker count changes after backend init

## Realtime Safety Checklist

The AudioUnit render callback must not:

- allocate
- lock mutexes
- block on channels or OS primitives
- print/log in hot path
- touch filesystem

Audit current callback path carefully for this after backend refactor.

Areas to review:

- message passing
- any hidden allocations in vector growth
- callback-time string formatting
- any one-time lazy initialization in callback path

## Testing Plan

## Stage 1: Functional

- one sampler track, no effects
- one sampler track, delay enabled
- one sampler track, `lexilush` enabled
- transport idle with effects inserted
- start/stop playback repeatedly
- rapid parameter changes while idle and while playing

## Stage 2: Scheduling

Compare:

- `cpal`, workers `0`
- `cpal`, workers `1`
- `cpal`, workers `N`
- AudioUnit, workers `0`
- AudioUnit, workers `1`
- AudioUnit + workgroup, workers `N`

Record:

- peak callback time
- overrun count
- glitch audibility

## Stage 3: Stress

- many tracks
- multiple custom effects
- delay and reverb heavy project
- hot-loading / effect replacement
- repeated start/stop of backend

## Rollout Strategy

1. Land backend abstraction with no behavior change.
2. Land AudioUnit backend behind macOS-only env flag.
3. Land instrumentation.
4. Land workgroup hookup.
5. Default macOS backend to AudioUnit once stable.
6. Revisit worker policy and default scaling.

## Recommended First Milestone

The first milestone should not be "full workgroup integration".

It should be:

- pure Rust AudioUnit backend
- single-threaded audiograph (`workers=0`)
- same sequencer behavior as today
- overrun instrumentation

That isolates:

- backend correctness
- callback correctness
- device lifecycle correctness

Then add workgroup-backed multithreading second.

## Expected Outcome

If this works, macOS behavior should improve in exactly the place the current engine is weakest:

- fewer callback deadline misses
- fewer idle-click / seam artifacts
- delay/reverb no longer exposing scheduler glitches as "impulses from silence"
- multithreaded audiograph becomes viable on macOS without introducing Swift

## Non-Goals

- rewriting the DSP engine
- removing the current C audiograph core
- making workgroups mandatory on non-macOS platforms
- replacing Cargo-based workflow

## Summary

This should be treated as:

- a backend replacement project
- a realtime host integration project
- not a DSP rewrite

The cleanest path is a pure-Rust macOS AudioUnit backend feeding the existing engine, with real workgroup hookup layered on after the backend is stable.
