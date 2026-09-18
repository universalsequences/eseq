# Editor housekeeping: live playback measurements

The first housekeeping pass (`eseq-y8np`) reduced observed main-thread CPU from
239.7 to 205.5 CPU milliseconds per wall second, **14.3%**, with approximately
60 displayed frames per second. The status-row change produced the clearest
saving. These measurements do **not** establish an additional CPU benefit from
the runtime metadata cache or shader notifications individually.

This follows the [visible renderer work](visible-renderer-performance-2026-09-18.md).
Audio processing and frame-rate policy were unchanged.

## Changes

- Status rows: compare borrowed status inputs for cache signatures without
  formatting text or building styled cells. Construct rows only when shown.
  Hidden rows still invalidate correctly when made visible; buffer name/dirty
  metadata remains part of frame invalidation independently of status visibility.
- Runtime context: retain buffer metadata, recency and visible effect membership.
  Compare their actual inputs so direct mutations of public buffer/tile fields
  cannot bypass invalidation. Copy active context strings only when changed.
  Hidden deferred effects and nil-returning projections preserve their behavior.
- Shader reload: use a native directory watcher on macOS instead of per-frame
  file metadata checks. Read source initially and after relevant notifications;
  unchanged contents do not recompile. Watching the parent supports atomic
  replacement and delete/recreate saves. Both replacement pipelines must compile
  before installation, preserving the previous pipelines on compilation failure.

The relevant source is `crates/eseqlisp/src/ui/frame.rs`,
`crates/eseqlisp/src/editor/runtime_context.rs`,
`crates/eseqlisp/src/ui/shader_watch.rs`, and their editor/backend integration.
The watcher uses the existing repository version of `notify` (6.1.1).

## Controlled workload and results

Each stage used two 18-second CPU-only samples with three seconds of warmup
per sample, followed by a separate nine-second presentation check with its own
warmup. The [live benchmark](ui-main-thread-benchmark.md) measures the actual
application main thread without a stack profiler. No builds ran during accepted
measurement windows.

The user restored playing `garageddd`, 19 tracks, the same layout and the same
instrument/FX panel for every accepted block. Recorded workload snapshots match:
2500 by 1700 physical pixels, 156 by 48 cells, selected track index 0, all seven
UI buffers, focused, visible and unoccluded. Every accepted sample had zero
input events and unchanged workload bookends.

| Build | Run 1, CPU ms/s | Run 2, CPU ms/s | Mean, CPU ms/s |
| --- | ---: | ---: | ---: |
| Before housekeeping | 230.0 | 249.5 | 239.7 |
| Status rows only | 198.4 | 200.4 | 199.4 |
| Status rows + runtime metadata | 212.1 | 201.5 | 206.8 |
| All three changes | 202.8 | 208.1 | 205.5 |

The combined observed saving is **34.3 CPU ms/s**, equivalent to about **3.43
percentage points of one CPU core**. It is not a 34-point Activity Monitor
reduction. Status-only measured 16.8% below baseline. Adding metadata increased
the mean by 7.4 CPU ms/s; adding shader notifications then decreased it by
1.3 CPU ms/s. With two short sequential samples per stage and the observed
spread, neither increment establishes a benefit. Eliminating known repeated
work is not sufficient evidence of a whole-main-thread improvement.

All four presentation checks measured approximately 60 FPS, with no missing
feedback or skipped presentations. Baseline presentation interval p50/p95/p99
was 16.67/25.00/25.00 ms; the combined build was 16.67/16.67/25.00 ms. These short
checks guard cadence, rather than establish improved presentation tails.
There was no input, so they cannot establish interaction latency or subjective
responsiveness. This was not a randomized crossover or confidence-interval study.

The current workload snapshot does not record actual FX owner/panel contents or
group expansion. Matching those relies on the user's explicit confirmation.
The user's separate 130% Activity Monitor observation was with a different
selected track inside the project and is excluded from performance attribution.
The cheaper 808 Clap panel is a useful workload comparison for subsequent work.

## Evidence and verification

Raw reports, exact values, exclusions, executable SHA-256 identities and
preserved binaries are in
`.local/benchmarks/renderer-pass-2026-09-18/housekeeping-comparison.json` and
its containing directory. Accepted baseline files are `housekeeping-before-7`
and `housekeeping-before-8`; earlier samples with focus/input changes or an
additional installed app are excluded. Baseline 4 is corroborating evidence
only and is not pooled into the accepted contiguous block.

The final executable is `housekeeping-all-metal_seq`; `target/release/metal_seq`
has the same SHA-256:
`e125a325b28717f4961b71a460dd577eeca153b1d972a8b0923212d11927acd9`.

Validation passed 21 focused nextest tests: 11 status/frame tests, seven runtime
metadata and deferred-effect tests, and three native watcher/Metal compilation
tests. Native watcher tests cover edits, atomic replacements and delete/recreate.
The release `metal_seq` build and `git diff --check` also passed. No full package
or workspace test suite was run.

The broader visible-UI objective (`eseq-6you`) remains open. Follow-up work is
tracked in `eseq-z09g` (record actual panel/group workload), `eseq-g46y` (retain
resolved drawing plans), and `eseq-dpi7` (cache UI discovery and port metadata).
