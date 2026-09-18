# Preserve unchanged paint within dirty subtrees — 2026-09-17

A dirty subtree used to replace every visited descendant's primitive storage
and paint revision. Even when the primitive cache returned identical geometry,
the new revision forced Metal to compile and upload that geometry again.

## Retention contract

The shared retained scene now observes inputs and output separately:

- For the built-in painters already eligible for primitive caching, unchanged
  props and paint context skip the painter. The observation owns nested list/map
  contents; shallow `Value` clones and hashes are not used as equality proofs.
  Known event callbacks and source metadata do not affect those painters.
  Live reactive values, unknown painters, opaque
  values, and cycles decline this shortcut.
- A painter that runs still retains its previous primitive allocation and paint
  revision when its output compares equal. This also handles changed inputs
  whose rendered result is unchanged, such as two values clamped to the same
  knob position. Head and tail runs have independent revisions, so an updated
  scrollbar preserves its unchanged clip run.
- Retained scenes call the painter directly instead of going through a second
  global primitive cache, hash, and vector clone. The legacy collection path
  continues to use that cache.

Widget state, shared state, hover, focus, theme, shader registry, layout geometry,
viewport metrics, and UI scale still invalidate as appropriate. Animation has an
explicit forced-paint path, and culled nodes keep their pending invalidation.
Live reactive props always go through the painter when dirty. Equal atomic
reads before and after painting cannot rule out an intermediate value observed
by the painter, so they are not used as an input-cache proof. Their unchanged
paint output still preserves the compiled geometry. Skipping their painters
would require the painter itself to consume a coherent frozen input snapshot.

`WidgetInstance.itime` is presentation data, normalized in retained storage:
Metal supplies current time in the vertex shader and the shared scene's
`flatten` method supplies it to wgpu. Every other uniform, including transition
start times, participates in paint comparison. Wavetable bank equality uses
immutable `Arc` identity plus the primitive's other fields/revision, avoiding a
scan of potentially millions of samples; replacing the bank counts as changed.

The Metal buffer lease/completion ownership rules are unchanged. Reusing a paint
revision lets the existing compiled-run cache retain its geometry safely.

## Release measurement

`retained_sparse_subtree_update_perf` renders a new layout allocation containing
96 knob/label pairs with literal value props each frame. One pair changes, but
the parent is marked dirty.
It warms 120 frames and measures 240 at 1600 × 1200 through the production tiled
renderer. CPU timing excludes the GPU completion wait; the probe excludes
audio, Lisp reevaluation, event dispatch, and display presentation.

Baseline: `1edbf5cb` plus the identical benchmark, built before the implementation.
The baseline and candidate test executables were saved and alternated on this
Apple M1 Max, with no Cargo builds, instrument compilation/validation, or app
playback running during the reported measurement. One contended trial was
discarded after detecting another instrument compiler; its reports are retained
under `contended-trial/`. The final runner checked for competing work before
and after each run.
The table reports the median of three run medians, except the p95 row, which
reports the median of three run p95s.

| Sparse dirty-subtree update | Baseline | Candidate |
| --- | ---: | ---: |
| Renderer CPU p50 | 0.359 ms | 0.167 ms |
| Renderer CPU p95 | 0.489 ms | 0.299 ms |
| Scene preparation p50 | 0.126 ms | 0.065 ms |
| Nodes painted per frame | 193 | 3 |
| Compiled runs per frame | 192 | 2 |
| Compiled cache hits per frame | 0 | 190 |
| Geometry uploaded per frame | 181,632 bytes | 1,892 bytes |
| New Metal geometry allocations per warm frame | 0 | 0 |

That is **2.15× faster renderer CPU time, 53.4% less time**, with **96× fewer
compiled runs and uploaded bytes** for this sparse update. Input comparison,
dirty-bound refresh, scene traversal, and drawing still take time.

| Repetition | Baseline p50 | Candidate p50 | Baseline p95 | Candidate p95 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 0.409 | 0.173 | 0.628 | 0.302 |
| 2 | 0.357 | 0.162 | 0.489 | 0.248 |
| 3 | 0.359 | 0.167 | 0.470 | 0.299 |

All times are milliseconds. Run-to-run and tail variation remain; these results
do not establish a fixed frame-time or tail-latency guarantee.

The existing all-changing-controls benchmark was also alternated three times.
Its median renderer CPU time was **0.568 → 0.537 ms**. All 192 runs still compile
and upload 181,632 bytes per frame, with zero new warm Metal allocations. This
checks that actual changes continue to update; it is not a large speedup for a
workload with no unchanged controls.

Raw samples, logs, the comparison runner, and saved binaries are under
`.local/benchmarks/retained-paint-2026-09-17/`. Binary SHA-256:

```text
baseline-tests   1c637f3d4f4c36f01fe767ce235bb58ead5ed97b9c41e651c66a5731a533dba6
candidate-tests  8275f4cdf4400902687590cdb765402acb045e8103acb17921cc97deda9da719
```

Reproduce the candidate measurement with:

```sh
ESEQ_RETAINED_BENCH_OUT=/tmp/retained-sparse.json \
  cargo nextest run --release -p eseqlisp --lib --run-ignored only \
  -E 'test(=ui::metal_backend::inner::render_dispatch_tests::retained_sparse_subtree_update_perf)' \
  --no-capture
```

## Validation and limits

Twenty-six focused release tests passed with `MTL_DEBUG_LAYER=1`. They cover
unchanged descendants, reactive values, mutable list/map cells, callback changes,
opaque/cyclic inputs, unchanged output after changed inputs, independent scroll
runs, animation time, wavetable bank identity, offscreen invalidation, hover,
focus, theme, bounds, clipping, atlas changes, and GPU buffer ownership.

The new production Metal pixel regression compares sparse updates against an
independent backend with a freshly painted scene and dynamic drawing. It cannot
silently reuse the retained primitives being checked. A local comparison over
the changed pair also proves the pixel threshold rejects its stale state; a
whole-frame average alone could hide one wrong control among 96 pairs. Existing queued-frame
tests still verify GPU lifetime safety, including the deliberately broken
ownership case that must visibly corrupt earlier frames.

`cargo check -p eseqlisp --lib --features wgpu,capture-harness` passed, with the
existing wgpu API deprecation warnings. No shaders, Lisp UI structure, or DSP
were changed by this task.

`cargo build --release -p sequencer --bin metal_seq` passed. The ten-track
`renderer-scroll.lisp` production capture was run at 2400 × 1400 with all panels
and a 220-cell FX scroll, then visually inspected. The transport, mixer, and
clipped FX contents render correctly. The image and capture log are under
`.local/benchmarks/retained-paint-2026-09-17/production-panels.png` and
`production-capture.log` respectively.

The original `impakt` custom UI override remains unidentified;
whole-app playback and scene interpolation still require a new live profile.

Live reactive props and unknown painters still run when dirty; identical output
can reuse their paint.
Layout/topology changes still rebuild the scene index. Input snapshots add
storage proportional to the observed props of painted nodes. There is no known
fragile workaround in this change.
