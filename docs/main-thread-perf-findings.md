# Main-thread performance implementation findings

Implementation date: 2026-07-31.

## Animation-frame ownership

Animation claimants fall into two groups:

- Layout-static: isometric event views with `auto-rotate`, phaser-notch displays
  with a nonzero amount, animated SDF widget definitions, and widgets using an
  animated SDF material.
- Runtime-state: piano-key press/release interpolation, matrix click
  interpolation, and pending/morphing patcher agent UI.

Layout nodes cache both groups bottom-up. Static animation answers are served
from the cached flags. Runtime-state branches remain eligible to change without
a layout rebuild, but the per-frame query traverses only those branches. Set
`ESEQ_DEBUG_ANIMATING_WIDGETS=1` to log the active widget type/id set whenever
it changes.

The always-on step animations were the Lisp SDF widgets `aqua-button` and
`tick`. Both now use a fixed selected-state offset and no longer register as
time-animated. The remaining sequencer Lisp SDF animations are transient: the
browser editor spinner exists only while a compile is pending, and the queued
scene pill exists only while a scene launch is queued.

Animating layouts use the retained primitive-run index. Each frame marks only
the active animation widget ids as paint-dirty, rebuilds those subtrees, and
reuses clean sibling runs.

## Backend event polling

`MetalBackend::poll_backend_event` has no substantial work of its own. It
checks the file-drop queue, then `poll_event` checks the coalesced event queues
before delegating to winit's `EventLoopExtPumpEvents::pump_events`. The profile's
zero self weight is therefore consistent with time below the winit/macOS event
pump, including the requested wait interval; there is no evidence-backed code
change to make at this seam.

Set `ESEQ_DEBUG_BACKEND_POLL=1` to emit one-second aggregates with:

- immediate queue hits;
- event-pump call and delivered-event counts;
- total requested wait duration; and
- event-pump wall duration.

This separates intentional event-loop waiting from unexpected pump overhead in
the next Instruments capture without adding logging work in normal builds.

## Visual regression

The retained animation-scene change was captured at 1800x700 against clean
HEAD `cbd26e75` using `arrangement-timeline.lisp`:

- `*mixer*`: before/after PNGs were byte-identical, SHA-256
  `cf4f5e414fe32e69b010a3293bc87d5defd4e84b80ea3f09d24c31458e997cbd`.
- `*arrangement*`: before/after PNGs were byte-identical, SHA-256
  `1d7c5e84c7290a3241aa2aeeb3b2cf3c5727d6e3ae75ff6d4b1b53573220efe1`.
