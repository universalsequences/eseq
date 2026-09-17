# Event-driven custom UI discovery

The `metal_seq` event loop used to rebuild its hot-reload file list every second.
That called `custom_ui_source_paths`, recursively traversing instrument, audio
effect, and MIDI effect directories on the UI thread, even during ordinary
playback and gestures. The earlier `impakt` profile attributed approximately
614 ms of accumulated samples to this discovery path; that was not a per-scan
latency measurement.

## Implementation

The `lisp-ui-discovery` worker owns the native filesystem watcher, directory
subscriptions, and custom UI index. It performs an initial discovery and then
sleeps on a condition variable. File events update individual UI/DSP pairs;
directory events reconcile the affected subtree. Unrelated file edits do not
recursively enumerate sibling instruments. The UI loop sends loaded Lisp paths
only when the runtime's source-graph revision changes, then takes pending change
batches without touching the filesystem during idle polling.

Recursive subscriptions cover content and package roots. Nonrecursive parent
subscriptions allow missing roots and source directories to appear later and
survive directory replacement. Package events refresh validated content roots,
including newly installed instruments/effects and removed manifests. Directory
subscriptions are installed before discovery; events arriving during scans stay
queued for the next pass.

Events are coalesced for 150 ms, with a one-second maximum delay under continuous
writes. This deadline exists only while events are pending; it is not a periodic
scan. Source-list updates do not bypass the event debounce window. The ingress
queue is bounded at 1,024 events. Overflow, native rescan flags, and watcher errors
request a full reconciliation on the worker rather than silently losing changes.
Shutdown wakes and joins the worker, releasing its native subscriptions.

Change batches retain the classification of deleted custom sources. Removing
`ui.lisp`, its `dsp.lisp` partner, a containing folder, or an eligible package root
therefore rebuilds generated dispatch without that source. A deleted file's
unmodified open buffer cannot resurrect it. Dirty buffers remain protected, and
explicit evaluation can still use an unsaved custom UI overlay. Paths normalize
their existing prefix so deleted `/var/...` files retain their `/private/var/...`
identity on macOS.

## Validation

Fourteen distinct focused tests passed: the following 13-test run, then all six
reload/identity tests after adding the deleted-file symlink regression.
The final identity change does not alter the worker or discovery index.

Focused validation command (now selects all 14):

```sh
cargo nextest run -p sequencer --bin metal_seq \
  -E 'test(/lisp_hot_reload::/) or test(=custom_ui::tests::custom_ui_reload_rerenders_existing_effect_buffer_body)' \
  --no-capture
```

Coverage includes incremental UI/DSP eligibility, directory moves, root removal,
bounded event storms, native loss flags, real atomic saves, initially absent and
recreated roots, loaded-source subscription updates, missing init-file parents,
installed package discovery, manifest removal, dirty/clean editor buffers,
deleted-file identity through symlinked parents, deleted custom UI dispatch,
and existing effect-buffer rerender behavior.

The idle regression creates 64 instruments, waits for initial discovery, then
performs 100,000 UI polls and waits another 1.2 seconds. On this M1 Max debug build,
the polls took **12.28 ms total** and caused **zero additional directory scans or
worker passes**. The timing is a small polling-cost observation, not a whole-app
frame-rate improvement or a release benchmark.

`cargo check -p sequencer --bin metal_seq` and the final
`cargo build --release -p sequencer --bin metal_seq` passed. The release build
took 3m 23s; `target/release/metal_seq` is ready for restart. Validation logs are
under `.local/benchmarks/hot-reload-discovery-2026-09-17/`.

## Scope

Actual Lisp evaluation and generated custom UI rebuilding still run on the UI
thread when eligible files change. Those generators still read their sources;
this change removes recurring discovery work during normal interaction. It does
not implement unchanged-paint retention or change DSP, rendering, or UI layout.

Native notification behavior was exercised on macOS. Linux uses the same worker
and `notify` abstraction but its native backend has not been executed here.
No known fragile workaround was introduced. A new live `impakt` profile is still
needed to quantify the effect on application frame-time tails.
