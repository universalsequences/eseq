# Scratch-only playback baseline

Follow-up to the steady playback pass, tracked as `eseq-nqbf`.

The user observed about 180% process CPU with the interface and 28 tracks,
and about 160% with only scratch. Their new main-thread profile still showed
380 ms in sequencer-view discovery and 281 ms in neural/track-event display
publication. Hiding panels had stopped most drawing but had not stopped all
preparation of their data. The parent event-poll stack is unexpanded in the
provided profile, so its time is not attributed to painting here.

## Changes

Sequencer-view discovery reads the current Lisp `defstate` through a read-only
runtime accessor. It does not invoke the tab-rendering accessor, which also
processes dirty reactive effects and flushes widget trees. This retains custom
step-tab registration, removal, and text-only visibility semantics. The actual
Tracker package integration test protects that contract.

Each neural/graph/event visualization publishes only when a visible effect,
widget binding, or explicit nonvisual observer consumes its field. Hidden
sources are not read or converted into Lisp values. A returning consumer gets
a current snapshot, including an empty one when its source stopped while
hidden. Dead sources clear once rather than publishing empty values forever.

Meter reads follow the existing panel visibility contract. Reopening a panel
forces a fresh sample without waiting for the meter interval. Hidden FX panels
release modulation snapshot subscriptions; an unobserved sampler waveform
releases its voice snapshot subscriptions. Track activity, transport playhead,
and browser preview displays also avoid hidden-only publication. Rack trigger
latches still drain so hiding a panel cannot leave a stale trigger pending.

The live event loop also avoids widget shader polling and SDF animation scans
without a widget layout. Rendering still compiles pending pipelines when a
panel returns. An absent learning preview no longer causes a runtime-context
refresh just to attempt clearing it.

Scratch remains a running application: event dispatch, transport, recording,
project/graph changes, latency compensation, and explicit script observers
continue. This is a minimal display baseline, not an audio-only executable.

## Validation and measurement

`cargo build --release -p sequencer --bin metal_seq` passes. The executable
is rebuilt; the author's running app was not restarted. Changes remain
uncommitted.

Three focused release tests pass:

- `state_values::tests::visualization_sync_requires_live_consumers_and_refreshes_reopened_panels`
- `state_values::tests::sequencer_visibility_reads_live_registry_without_invoking_lisp`
- `state_values::tests::tracker_package_import_installs_a_main_panel_tab_and_edits_steps_by_key`

The saved-project replay uses the frozen 19-track `garageddd` snapshot from the
previous report plus nine blank sampler tracks, giving 28 tracks. It does not
modify the saved project or the author's running session. It drives changing
playheads, process histories, compressor histories, and track-event telemetry,
and forces display polling due on every iteration. This deliberately exercises
work the original fast replay mostly missed between its wall-clock poll deadlines.

Two alternating before/after release runs on the M1 Max discard 30 warmup
iterations and retain 180 observations per phase per run. The comparison is
against the previous UI fixes already applied, not original repository HEAD.
No Cargo build ran during these measured replays.

| Scratch-only sync | Before | After |
| --- | ---: | ---: |
| Median | 0.671 ms | 0.034 ms |
| p95 | 1.396 ms | 0.103 ms |
| p99 | 2.104 ms | 0.283 ms |
| Steady redraw requests | 0 / 360 | 0 / 360 |

Median synchronous UI/control work fell by about 95%. Both versions already
avoided steady redraws in this replay; the gain removes work between frames.
The candidate asserts unchanged hidden event/activity/scope/playhead fields,
released display subscriptions, no scratch redraw requests, and fresh meter
sampling when the real panel layout returns. Scratch, live-panel, and reopened
panel PNGs were inspected.

These are elapsed durations of production synchronization, frame building, and
Metal submission, not Activity Monitor percentages or audio deadline results.
The scratch samples need no frame build or submission. The replay excludes OS
event polling, shader-watch polling, live analyzer polling, and real scheduler
execution. The two small live-loop idle-poll changes are therefore outside the
reported timing gain. Matched live process-CPU validation remains `eseq-yjbz`.

Evidence is in `.local/benchmarks/scratch-ui-2026-09-18/`: the pasted profile,
`before{,-2}.json`, `after{,-2}.json`, `comparison.json`, focused test logs,
replay logs, PNGs, and the preserved before-replay executable.

```sh
ESEQ_UI_REPLAY_PROJECT="$PWD/.local/benchmarks/garageddd-ui-diagnosis-2026-09-17/garageddd-replay.json" \
ESEQ_UI_REPLAY_EMPTY_TRACKS=9 \
ESEQ_UI_REPLAY_OUT=/tmp/scratch-ui-replay.json \
cargo nextest run --release -p sequencer --bin metal_seq \
  -E 'test(=tests::saved_project_ui_playback_replay)' \
  --run-ignored only --no-capture
```

For the live comparison, restart into the rebuilt release, keep the same
project/scene/playback state, and compare full panels with scratch alone after
the transition settles. The difference measures display-related work; the
remaining process CPU includes audio, scheduler, and application control work.
