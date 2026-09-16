# Metal geometry storage reuse — 2026-09-16

The `impakt` playback/scene-interpolation profile exposed allocation work that
quiet scrolling did not exercise: 705 ms creating Metal buffers inside 904 ms of
run compilation, and 399 ms tearing down replaced compiled runs. Updating a
paint revision previously allocated every command's GPU storage again.

## Ownership and reuse

`ui/metal_buffer_pool.rs` gives each compiled draw command an immutable buffer
lease. Every encoding scope pins the leases it draws, including cache hits and
repeated clipped/phase draws. Submission transfers those leases to a completion
queue with the exact command buffer that uses them. Dropping an unsubmitted scope
releases its pins without publishing a GPU submission.

Cache replacement removes its obsolete owner before uploading the new data.
Storage returns to the pool only when **all cache, CPU, and GPU frame owners have
released it**. The pool releases submitted frame ownership only at Metal's
terminal `Completed`/`Error` status. Completion of one reader does not permit
reuse while another reader remains. Keeping a Metal object alive alone would not
prevent CPU writes to its contents; the leases protect those bytes as well.
This follows Apple's [CPU/GPU synchronization contract](https://developer.apple.com/documentation/metal/synchronizing-cpu-and-gpu-work).

Free storage is grouped by power-of-two capacity, with a 256-byte minimum.
Uploads copy the current payload into an exclusively owned spare and preserve
the draw's actual vertex/instance count. Growth acquires a larger capacity;
smaller payloads can reuse the same capacity. The pool does not reinterpret
old values, skip updates, or depend on a guessed number of elapsed frames.

Resource limits are separate:

- The existing retained cache allows 8,192 logical runs / 64 MiB of capacity.
- The spare pool retains at most 8,192 buffers / 64 MiB; excess spares are freed.
- At most three submitted frames retain leases. If the GPU falls behind, encoding
  waits for the oldest submission instead of growing the frame queue indefinitely.

These are not a 64 MiB limit on total rendering memory: cached storage, spare
storage, and older in-flight versions can coexist. Pool accounting uses allocated
capacity, including size-class rounding. CPU primitive generation and copying
updated bytes remain part of rendering.

## Validation

Seven focused tests passed with `MTL_DEBUG_LAYER=1`:

- Capacity reuse, changed payload sizes, and cache ownership that prevents reuse.
- Abandoned encoding scopes, per-frame pin deduplication, and spare budgets.
- A real GPU fence test with two readers: completing the first reader while
  keeping the second blocked must not recycle their shared storage. Readbacks
  verify each submission's original bytes.
- Continuous updates to 96 knobs and 96 labels, with zero warm storage
  allocations and pixels compared with the dynamic renderer.
- Existing fractional-scroll, hover, and atlas-invalidation regression; changed
  controls and atlas UV updates now reuse storage while refreshing draw data.
- Three production-renderer frames queued behind a GPU fence, with the compiled
  cache evicted between frames. Every resulting image matches its own values.
- A fault case deliberately discards GPU ownership using a test-only helper.
  It corrupts the earlier queued images, and the pixel regression detects it;
  the final image still contains the newest values. The helper is absent from
  app builds.

The tiled encoding function returns its submitted command buffer internally;
the public synchronous capture wrapper owns the completion wait and readback
timing. Tests can therefore exercise the actual queued production renderer.

`cargo check -p eseqlisp --lib --features wgpu,capture-harness` passed. No shaders,
Lisp UI structure, or audio processing were changed by this pass.

`cargo build --release -p sequencer --bin metal_seq` passed. A production capture
using `renderer-scroll.lisp`, all panels, and a 220-pixel FX scroll was inspected
at 2400 × 1400: the transport, mixer, and clipped FX contents render correctly.
The image is `.local/benchmarks/renderer-storage-2026-09-16/production-panels.png`.

## Measurement

The ignored release test
`ui::metal_backend::inner::render_dispatch_tests::retained_value_update_perf`
renders 96 changing knobs and 96 changing labels at 1600 × 1200. It warms 120
frames and measures 240 frames, through the production tiled renderer. This
isolates an update-heavy workload; it excludes audio, input dispatch, and display
presentation. Readback waits serialize GPU submissions in this timing probe;
the independent fence tests above verify overlapping submissions.

The baseline test binary was built from `6c4cf31b` plus this same ignored test,
before adding the pool. Saved binaries and per-frame reports are under
`.local/benchmarks/renderer-storage-2026-09-16/`. The comparison alternates old
and new binaries for three repetitions, with no concurrent Cargo builds.

On this Apple M1 Max, the median of the three run medians improved from
**1.549 ms to 0.626 ms: 2.48× faster renderer CPU time (59.6% less time)**.
Each baseline repetition allocated **69,120 Metal geometry buffers** over its
240 measured frames; every candidate repetition allocated **zero**. Each
candidate frame reused 288 buffers and uploaded 181,632 bytes of changed geometry.
Scene preparation stayed approximately unchanged (0.267 → 0.266 ms).

All repetitions, in milliseconds:

| Repetition | Baseline CPU p50 | Candidate CPU p50 | Baseline CPU p95 | Candidate CPU p95 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 1.582 | 0.626 | 2.016 | 0.810 |
| 2 | 1.548 | 0.647 | 1.968 | 1.719 |
| 3 | 1.549 | 0.578 | 2.071 | 0.808 |

Candidate repetition 2 had a higher tail, including a 13.45 ms maximum frame;
the cause was not identified. It still allocated no Metal geometry buffers.
No candidate measurement waited for storage backpressure; the serial capture
probe held one submitted frame at the point its timing was recorded. This does
not measure the live app's three-frame backpressure behavior or guarantee its
frame-time tails.

The saved binaries' SHA-256 hashes identify the compared implementations:

```text
baseline-tests   25f51b9e351f379ad6d99fd03fd74f2af2ad0f27d6fae884f28684fa07a7908f
candidate-tests  2e7c7acc509e4c702c2ff3d3a874e8560bfa97da4d6469e02c05733e806d1459
```

Per-frame JSON, run logs, and `comparison.json` are alongside those binaries.
The direct-binary comparison runner imported the environment defaults from
`.cargo/config.toml`, including the repository's test stack budget.

Reproduce a candidate measurement with:

```sh
ESEQ_RETAINED_BENCH_OUT=/tmp/retained-values.json \
  cargo nextest run --release -p eseqlisp --lib --run-ignored only \
  -E 'test(=ui::metal_backend::inner::render_dispatch_tests::retained_value_update_perf)' \
  --no-capture
```

The capture JSON also reports storage reuses, uploaded geometry bytes, spare
bytes, in-flight frame count, and time waiting for storage backpressure. That
wait is reported separately from renderer CPU time. The live
`ESEQLISP_PROFILE_UI` log includes storage reuses and geometry upload volume.

## Remaining scope

The original custom UI override has not been identified, so this is not a replay
of `impakt`'s complete scene gesture. Its retained-scene preparation, reactive
synchronization, and periodic hot-reload discovery remain separate performance
work. Whole-app improvement must be measured in a new live profile.

No known fragile workaround was introduced. The GPU lifetime regressions run on
this Mac's Metal device; they do not certify every GPU/OS combination.
