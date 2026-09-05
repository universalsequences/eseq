# Sound Evolve — Synplant-style parameter discovery

**Status:** rev 1, 2026-09-03. Design only; nothing built.
**Sibling:** `docs/patch-learn-spec.md` (backprop trainer, the "Genopatch with
gradients" path). This spec deliberately does NOT use Adam/backprop. It reuses
the trainer's process model, transport, seed/bounds discipline and excitation
convention, and adds a new dgenlisp subcommand that only does batched forward
rendering + reductions.

## 1. Goal

A "…" menu item on any custom instrument turns the `*mixer*` buffer into a
temporary sound discoverer: a center sound (the instrument's current knobs)
surrounded by a ring of candidates that are meaningfully different from it and
from each other. Click a candidate to audition it through the real engine.
Click again to make it the new center and grow a fresh ring. Optionally drop a
sample to aim the search at it. Keep writes one undoable change; exit throws
everything away.

Two things Synplant gets right that we keep:

- **No sample required.** The human ear is the fitness function. The tool's
  job is to spend compute making sure every branch is worth the click.
- **One interaction.** Audition, pick, repeat. No optimizer knobs in the main
  flow.

One thing we add that Synplant 1 cannot do: the ring is not 12 random
neighbors. We render hundreds or thousands of children per round in parallel,
compute descriptors for each, and show the 12 that are best spread out (or,
with a sample, the 12 closest to it). Parallelism buys *diversity* when there
is no target and *convergence* when there is.

## 2. Architecture

```
eseq (host, Rust)                          dgenlisp evolve (Swift, GPU)
─────────────────────────────              ─────────────────────────────
  seed = current knob values   ──────────►  render B candidates (batched fwd)
  bounds = @min/@max           request      STFT per candidate (on-GPU)
  target sample (optional)                  reduce → per-candidate row:
  candidate param matrix                      distance(target)?, descriptors[8],
                                              loudness/clip/silence flags
  ◄──────────────────────────────────────── rows (≈10 floats each), NDJSON
  filter degenerate rows
  normalize descriptors
  select ring (farthest-first / grid / top-K)
  CMA update (sample mode) → next matrix
  render the 12 winners (GPU re-request
    with --emit-wavs) → audio for audition + glyph
```

Division of labor is decided by bandwidth, not compute: 1000 × 1 s × 48 kHz is
~190 MB per generation, so waveforms never come back in bulk. The GPU renders,
computes the STFT, reduces, and returns tiny rows. Rust does everything serial
and cheap: filtering, normalization, selection, the CMA step, the UI.

**GPU only.** Patches the lowering pass cannot express (sync, ring mod, and
whatever else `plan` reports as `unsupported`) are refused: the menu item runs
`--plan-only` first and the buffer shows "this instrument can't be evolved:
unsupported node X" instead of a ring. No CPU fallback in v1; a CPU-dylib
batch path is tabled. A small Rust descriptor implementation still exists,
but only as the test oracle for the GPU reductions (§8), not as a renderer.

## 3. `dgenlisp evolve` — CLI contract

```
dgenlisp evolve <patch.lisp> \
  --seed seed.json \
  [--target target.wav] \
  --candidates candidates.json | --generate N --sigma S [--rng-seed K] \
  --job-dir <dir> \
  [--seconds 1.0] [--pitch-hz 110] [--gate-frames 8820] [--batch 0]
```

- **Stateless per invocation.** One call = one generation. The host owns the
  loop, the CMA state, and the selection policy. This keeps the Swift side a
  pure "render + reduce" kernel and lets the host swap strategies without
  touching dgenlisp.
- **`--candidates`** is an explicit param matrix in natural/knob units, keyed
  by param name (the same discipline as `seed.json`, §3.1 of patch-learn).
  **`--generate N --sigma S`** is a convenience for round one: Gaussian noise
  in normalized 0..1 space around the seed, clipped to bounds, enum/switch
  params flipped with probability `S`. The trainer echoes the exact matrix it
  rendered in the `plan` event either way.
- **Bounds** are `@min`/`@max` on `param` nodes, as in patch-learn. Missing
  bounds → refuse-and-report for that param (it is frozen at seed value and
  listed under `frozen`).
- **Excitation** is patch-learn §6 verbatim: single trigger at t=0, gate
  hard-coded on for `gate_frames`, fixed pitch, velocity 1.0. With a target,
  pitch and gate default from the target's f0 and envelope; without one, from
  the host's audition note.
- Exit 0 iff a terminal `result` event was emitted.

### 3.1 Event protocol (NDJSON on stdout, chatter on stderr)

```json
{"type":"plan","learnable":["cutoff","res","decay"],
 "frozen":[{"name":"base_note","reason":"no-bounds"}],
 "unsupported":[],"seed_echo":{"cutoff":0.4,"res":0.2,"decay":0.35},
 "candidates":1024,"pitch_hz":110.0,"gate_frames":8820,"crop_frames":48000}
{"type":"progress","done":512,"total":1024}
{"type":"rows","path":"<job-dir>/gen0003.rows.json"}
{"type":"result","generation":3,"rows":"<job-dir>/gen0003.rows.json",
 "best_distance":0.0142}
{"type":"error","message":"..."}
```

Rows go to a file, not the stream (1024 rows × ~12 fields is small but not
"a line"). Row shape:

```json
{"i":17,
 "params":{"cutoff":0.61,"res":0.12,"decay":0.35},
 "distance":0.0142,                 // absent without --target
 "desc":{"centroid":7.9,"centroid_slope":-1.2,"attack":0.004,"decay":0.41,
         "flatness":0.08,"harmonicity":0.71,"flux":0.13,"crest":9.2},
 "rms_db":-14.3,"peak":0.82,"clipped":false,"silent":false}
```

`params` are echoed per row in natural units so the host never has to
re-derive which matrix row a result belongs to (the `naturalValues()` bug
class, again).

### 3.2 Distance

Same multi-resolution STFT distance the trainer uses, so a number here means
the same thing as a number in the learn pane. Report absolute distance;
"improvement %" is computed by the host against the seed's own row.

## 4. Descriptors (DECIDED set for v1)

Each is one scalar per candidate, computed on the GPU from the STFT and the
RMS envelope already needed for the distance. Reference implementation in
Rust (`sound_glyph::extract` already computes several of these for glyphs;
share, do not duplicate).

| name | what it measures | how |
|---|---|---|
| `centroid` | brightness | energy-weighted mean frequency, log2 Hz, over the whole clip |
| `centroid_slope` | brightness movement | centroid of first 20 % minus centroid of last 20 % (positive = darkens) |
| `attack` | percussive vs swelling | seconds from onset to RMS peak |
| `decay` | short vs long | seconds from RMS peak to −40 dB (clamped to clip length) |
| `flatness` | noise vs tone | geometric / arithmetic mean of the spectrum |
| `harmonicity` | pitched vs inharmonic | normalized autocorrelation peak height at estimated f0 |
| `flux` | static vs moving | mean frame-to-frame spectral change |
| `crest` | spiky vs dense | peak / RMS, dB |

Plus three gating fields that are not descriptors: `rms_db`, `peak`,
`clipped`, `silent`. These exist so the host can drop degenerate children
before selection. Descriptors spread survivors out; they never judge.

## 5. Host loop (Rust)

Per round:

1. Build the candidate matrix. Round one: `--generate N --sigma S` where N is
   the batch the time budget allows (default 512) and S is the mutation
   radius from the UI. Later rounds in sample mode: CMA-ES sampling around the
   current mean with the CMA covariance; in seedless mode: fresh Gaussian
   around the new center.
2. Run `dgenlisp evolve`. Consume `plan` first (show frozen/unsupported
   before any wait), then progress, then rows.
3. Filter: drop `silent`, `clipped`, `rms_db` below the seed's by more than
   24 dB, `decay` pinned at clip length (did not decay).
4. Normalize descriptors to zero-mean unit-variance across the survivors.
   Locked params (§7) are already constant, so they contribute nothing here.
5. Select the ring, by mode:
   - **Seedless:** farthest-first. First pick = farthest from the center;
     each subsequent pick maximizes its minimum distance to everything already
     chosen. 12 picks.
   - **Sample:** top-12 by distance, then de-duplicate: any pick closer than
     ε (normalized descriptor space) to a previous pick is replaced by the next
     best. Keeps the ring from being 12 copies of the same local optimum.
   - **Steer (v2, §9):** distance to a synthetic descriptor target, same
     selection as sample mode.
6. Render the 12 winners for audition and glyphs: re-invoke with
   `--candidates` of 12 and `--emit-wavs`. Twelve renders are free.
7. Time budget, not generation count: default 3 s of GPU per round, cap 8
   generations in sample mode, 1 generation in seedless mode (seedless has no
   notion of convergence; more generations would only mean a bigger pool).

Each round's `rows.json` and matrix live under `evolve-jobs/<id>/gen%04d/` so
a ring can be replayed or diffed after the fact, mirroring `learn-jobs/`.

## 6. UI — the evolve buffer

Entry: "Evolve…" in `instrument-header-action-options`
(`content/ui/effects/panel-frame.lisp`), offered for the same instrument
kinds that get "Edit" (custom instruments; not sampler, modulator, rack).
The handler issues a host command that swaps `*mixer*` for `*evolve*` on that
track and records how to come back.

The buffer is a single view with no state machine beyond "empty / growing /
ring". Rough layout:

```
┌──────────────────────────────────────────────────────────────────────┐
│ ✕ exit     Evolving  Digiwave · track 3          [keep]   mutation ◉─── │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│                 ◌         ◌         ◌         ◌                      │
│                                                                      │
│            ◌          ┌───────────┐          ◌                       │
│                       │  center   │                                  │
│            ◌          │  (glyph)  │          ◌                       │
│                       └───────────┘                                  │
│                                                                      │
│                 ◌         ◌         ◌         ◌                      │
│                                                                      │
│   ┌─────────────────────────────────────────────┐                    │
│   │  drop a sample here to aim at it  (optional) │   ▶ regrow         │
│   └─────────────────────────────────────────────┘                    │
├──────────────────────────────────────────────────────────────────────┤
│  locks:  [cutoff 🔒] [res] [decay] [drift 🔒] [env2] …               │
└──────────────────────────────────────────────────────────────────────┘
```

Behaviors:

- **Center** shows the current knob values as a sound glyph (the
  `sound_glyph` extraction library) and plays them on click.
- **Ring** shows 12 candidates, each a small glyph. Hover = audition through
  the running engine via the preview layer; the knobs in the sidebar visibly
  move to the candidate's values while hovered and snap back on leave. This
  is the same preview-layer mechanism patch-learn §8.3 needs (bypass document
  and undo; the only document mutation is Keep). Build it once here.
- **Click a branch** = it becomes the center; a new ring grows around it.
  The previous center is pushed onto a breadcrumb row under the header so you
  can back out a step. Breadcrumbs are ephemeral (die on exit).
- **Ring placement** is meaningful in seedless mode: candidates are laid out
  on the ring by angle from their 2-D projection (centroid on x, decay on y)
  so bright sounds cluster on one side and long sounds on the other. In sample
  mode the ring is ordered by distance clockwise from the top.
- **Drop zone.** Accepts the browser's existing `"sound"` drag type
  (`content/ui/browser.lisp` already emits it). Because we are in the mixer
  buffer, the sidebar browser is available without any samples-only mode.
  Dropping switches the loop to sample mode, shows the target's glyph next to
  the drop zone with a "×" to clear, and regrows immediately. The center's
  distance to the target is shown under the center; each branch shows its own
  distance as a small bar.
- **Mutation** knob = sigma for the next regrow, 0.02..0.5 in normalized
  units. Sample mode ignores it after round one (CMA owns sigma).
- **Locks.** One toggle per learnable param. Locked params are held at the
  center's value in every candidate. Frozen/unsupported params from `plan`
  render as locked-and-disabled with the reason on hover.
- **Regrow** re-runs the round from the current center without changing it
  (new random pool). Cheap "show me twelve more".
- **Keep** writes the center's params to the instrument as one undo
  transaction and returns to `*mixer*`. **Exit** returns without writing;
  the preview layer is cleared so the running engine snaps back to the
  document's values. Both also cancel any in-flight `evolve` process.
- **Growing state.** While a round runs, the ring shows placeholders with the
  `progress` counter; a stale ring is never shown as if it were fresh. Job
  death without a terminal event renders as "evolve died", never a spinner.

## 7. Locks, bounds, and what is searchable

- The search space is exactly the `param` nodes with `@min`/`@max`, as in
  patch-learn. Same refuse-and-report policy for missing bounds.
- Enum/select params (wave shape, algorithm selectors, filter modes) are
  searchable: mutation flips them with probability sigma; CMA treats them as
  categorical with per-value logits. Skip in v1 if it complicates the Swift
  side; list under `frozen` with reason `"enum"` and revisit.
- Params with `@mod` inputs or p-locks are searched at their base value; the
  preview shows them as the candidate's base with modulation still applied by
  the running engine.

## 8. Verification

- **Descriptor parity:** a fixture of ~20 WAVs, Rust oracle descriptors vs GPU
  descriptors within tolerance. Blocks shipping.
- **Distance parity with the trainer:** same target, same candidate, same
  number from `evolve` and `train --plan-only`-style scoring.
- **Round-trip:** after Keep, render the instrument through eseq's own engine
  and check the descriptors match the chosen row's within tolerance. All
  three worst SynthId bugs were trainer↔render plumbing; this is the guard.
- **Preview isolation:** hovering 50 branches and exiting leaves the undo
  stack unchanged and the document's params bit-identical.
- **Ring diversity:** farthest-first on a 512-candidate pool yields a minimum
  pairwise normalized distance above a threshold; a regression test on a
  fixed rng seed.

## 9. Later / open

- **Steer mode.** With descriptors in hand, a target does not have to be a
  sample: "brighter / longer / noisier / punchier" nudges or a few descriptor
  sliders make a synthetic target vector for the same selection code. This is
  the sample-free version of aiming and is the most promising v2 feature.
- **MAP-Elites grid** instead of a ring: bin the pool on two axes, show the
  best per cell, lay them out as a grid. Better once the pool is large; the
  ring reads better for 12.
- **Seed the seedless ring from a library** rather than noise: render a
  handful of existing presets of the same instrument and use them as the
  initial CMA mean set. Cheap way to get "musical" first rounds.
- **CPU-dylib batch path** (compile once, one memory block per candidate,
  rayon; the `tools/audition/audition.py` pattern) for patches the lowering
  pass refuses. Tabled by decision 2026-09-03; v1 just rejects them.
- Measure the FDTD drums on the GPU path; heavy patches may need a smaller
  default batch.
- Whether hover-audition should retrigger on every hover or gate behind a
  short dwell to avoid machine-gunning while sweeping the ring.
- Whether Keep should also write a preset file so a discovered sound is
  recoverable if the user later undoes.
