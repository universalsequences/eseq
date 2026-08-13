# Patch Learning ("SynthId in the editor") — Spec rev 1

Status: DESIGN — nothing built. Companion to the dgen repo's
`Examples/SynthID/SUBTRACTIVE_SPEC.md` (E4 "direction-finding mode"), which this
feature productizes. This document owns the cross-repo protocol; the dgen repo
implements the trainer side, eseq implements the UI/host side.

## 1. Goal

A mode in the patch editor that splits the pane: pick a target sample, spawn a
learning job against the currently edited instrument patch, watch progress live,
and get back per-param deltas that can be auditioned (A/B) and applied to the
patch's `param` nodes.

The contract is E4's: **projection onto the model manifold, not exact
recovery**. We gate on sound, never on parameter truth (per `E3_FINDING.md`:
basin discovery and refinement are solved; near-null degenerate directions —
res/shape, fAmt·fDecay — are objective-identifiability limits, and human ears
barely distinguish the compensated points anyway). Real analog targets plateau
around ~50% MR-STFT improvement; that is a *good* interactive result, not a
failure.

Evidence this works end to end (dgen repo):

- 808 three-rung ladder passed (84.55% on a real TR-808 recording).
- E2 polyblep-equivalence PASSED 5/5 at ~1.6e-7 MR-STFT distance between the
  training oscillator and the actual eseq `polyblep` lisp macros — the license
  that recovered knobs mean the same thing at deployment ("the fit topology is
  the deployment topology").
- Hoodie bass was ported into a real eseq instrument with `instrument_probe`
  passing.

## 2. Architecture

```
eseq (Rust, UI host)                    dgenlisp CLI (Swift dgen repo)
┌─────────────────────┐   spawn+argv    ┌──────────────────────────────┐
│ patch editor pane   │ ──────────────► │ `dgenlisp train`             │
│  - job manager      │                 │  - lisp parse                │
│  - progress widgets │ ◄────────────── │  - training lowering pass    │
│  - A/B + apply      │  NDJSON stdout  │  - Metal autograd trainer    │
└─────────────────────┘                 └──────────────────────────────┘
            │                                        │
            └────────────── job dir ─────────────────┘
                 (artifacts: wavs, checkpoints, events.jsonl)
```

eseq stays dumb: spawn process, stream events into a reactive cell, render
results. The trainer is fully exercisable from the shell without eseq — every
nasty SynthId bug (BatchRefine wrong-f0 scoring, `naturalValues()` dropping
frozen scalars, trainable-`statefulPhasor` gradient corruption) lived on the
trainer↔render seam, so keeping that seam narrow and shell-testable is a design
requirement, not a convenience.

## 3. CLI contract

```
dgenlisp train \
  --patch <dsp.lisp> \
  --target <sample.wav> \
  --seed-params <seed.json> \
  --job-dir <dir> \
  --mode direction \            # v1: E4 seeded short run + cold basin check
  [--epochs 300] \
  [--gate-frames N] \           # default: derived from target envelope, see §6
  [--pitch-hz F]                # default: CPU-estimated from target, see §6
```

- `--mode direction` (v1 only mode): seed from the user's patch, ~100–300
  epochs, small LR, plus one background cold restart as a basin check. If the
  cold restart decisively beats the seeded run, report `"basin_check":
  "wrong_neighborhood"` instead of pretending the deltas are meaningful.
- Future `--mode full`: stratified basin search (batched forward, ~13.6
  ms/candidate at B=256 — 1024 candidates ≈ 14 s) + full refinement schedule.
- Exit code 0 iff a terminal `result` event was emitted; nonzero otherwise.
- Cancellation = SIGTERM/kill. Trainer is stateless from the host's
  perspective; artifacts on disk survive.

### 3.1 Seed params travel explicitly

The lisp file's `param` defaults are NOT the user's current knob positions.
eseq dumps live values to `seed.json`:

```json
{ "params": { "sinefm": 0.06, "ratio": 0.05, "sixteenth": 0.36 } }
```

Bounds come from `@min`/`@max` on the `param` nodes in the patch source — those
declarations ARE the search space. The trainer echoes back the exact seed it
parsed (in the `plan` event) so a units/transform mismatch shows up as a
visible diff, not a silent wrong start (the `naturalValues()` bug class).

## 4. Event protocol (NDJSON on stdout)

One JSON object per line on **stdout**, each with a `type` field. All
human-readable/debug chatter goes to **stderr** — a stray print can never
corrupt the protocol. Small events on the stream; heavy artifacts (WAVs,
spectrogram frames) written to the job dir and referenced by path.

```json
{"type":"plan","learnable":["sinefm","ratio"],
 "frozen":[{"name":"base_note","reason":"f0-adjoint-unreliable"}],
 "unsupported":[],"seed_echo":{"sinefm":0.06,"ratio":0.05},
 "pitch_hz":49.2,"gate_frames":8820,"crop_frames":32768}
{"type":"stage","name":"train","total":300}
{"type":"epoch","epoch":50,"total":300,"loss":0.104,
 "params":{"sinefm":0.061,"ratio":0.048}}
{"type":"checkpoint","epoch":100,"wav":"<job-dir>/epoch0100.wav"}
{"type":"result","improvement_pct":54.2,"abs_distance":0.0116,
 "basin_check":"ok",
 "deltas":{"sinefm":{"from":0.06,"to":0.11},"ratio":{"from":0.05,"to":0.02}},
 "final_wav":"<job-dir>/final.wav"}
{"type":"error","message":"..."}
```

Rules:

- **`plan` is the first event**, before any compute: the lowering pass's
  verdict on this patch. The pane shows which knobs will move before GPU time
  is spent; "unsupported node X" is a first-class fast failure.
- **A terminal event always** — `result` or `error` as the last line. Host also
  watches exit code; process death with no terminal event renders as "job
  died", never an infinite spinner.
- Always report **absolute distance alongside improvement %** (the 808's
  84.55% → 77.53% corrected-baseline lesson).
- Cadence: a few events/sec at 0.27–1 s/epoch; `checkpoint` (preview WAV) every
  ~25 epochs.

## 5. Job identity & directory

Host creates `<repo-or-user-data>/learn-jobs/<job-id>/` and passes it via
`--job-dir`. Trainer treats it as its sandbox and writes:

```
learn-jobs/<id>/
  request.json      # host: argv, patch path, target path, seed, timestamps
  patch.lisp        # host: snapshot of the source as sent
  seed.json         # host
  events.jsonl      # host appends the consumed stream (replay/reattach)
  lowered.lisp      # trainer: the annotated graph it actually trained on
  epoch*.wav        # trainer: preview renders
  final.wav         # trainer
  result.json       # trainer: the terminal result event
```

This buys: reattach after app restart, replay for debugging, and — critically —
the artifact trail to diff "what the trainer trained" (`lowered.lisp`) against
"what the editor sent" (`patch.lisp`) when a result sounds wrong.

## 6. Excitation convention (DECIDED)

An instrument patch has `in gate` / `in trigger` / `in pitch` / `in velocity`
inlets; the trainer must drive them:

- **Single trigger at t=0.** One-shot excitation, one voice.
- **Gate hard-coded on for N frames** (`--gate-frames`), default derived from
  the target's amplitude envelope (gate off where the envelope falls below
  threshold — the release-point estimate). Hard-coding sidesteps the
  sustain-vs-decay ambiguity for v1.
- **Pitch frozen to the CPU-estimated f0 of the target** (autocorrelation/YIN
  as in `PitchTrack.swift`), overridable via `--pitch-hz`. Pitch is NEVER a
  learned parameter: the swept-pitch `f0` adjoint fails fdcheck (~0.47 rel
  error), and "what note was this rendered at" is exactly the bug that ate
  three BatchRefine polish rounds.
- Velocity: constant 1.0 for v1.
- Crop length from the sample (bounded by trainer frame limits).

The chosen values are echoed in the `plan` event so the host can display and
persist them.

## 7. Training lowering pass (dgenlisp side)

The lisp-level rewrite whose job is **transcription + policy, not smoothing**
(dgen's posture is no smooth surrogates — fix adjoints in the library). Per
node:

1. **Transcribe verbatim** known-good macros/ops (polyblep family, svf, biquad,
   accum-built envelopes, tanh/softsign saturation, phasor, history — per the
   fdcheck record).
2. **Freeze** params whose gradients are known-bad:
   - swept `f0` (fdcheck fail);
   - trainable `statefulPhasor` frequency (corrupts *other* params' gradients
     ~10×; pinned reproducer `SVFBPTTScratchTests…TrainableDetune`) — where a
     detune-style param matters, substitute the closed-form phase-offset trick
     `wrap(phase1 - t·detune)`.
3. **Refuse** (report in `plan.unsupported`) genuinely hard discontinuities:
   oscillator sync, ring-mod-style cases per the declared non-goals. A patch
   with unsupported nodes in the signal path can still run with those params
   frozen where that's sound; otherwise the job fails fast at `plan`.

Loss: multi-resolution log-magnitude STFT L1 (windows 256–2048, ε=1e-3) exactly
as frozen in `SPEC.md` §4; waveform MSE and friends remain banned. Adam with
per-group LRs in transformed coordinates; ~2× above the legacy production LRs
per `BATCH_REFINE_FINDING.md`.

## 8. eseq pane behavior

- Split the patch-editor pane (as mocked): job launcher (sample picker from
  library or file drop) + live progress region.
- Background thread: `BufReader::lines` on child stdout → parse → channel →
  reactive cell; the pane is an ordinary widget bound to that cell. Reuse the
  agentic-bubble subprocess-streaming seam.
- Display: loss curve; per-param delta arrows **on the actual param widgets**
  (knob ghosts from old→new value); spectrogram diff of target vs latest
  checkpoint render; frozen/unsupported params marked from the `plan` event.
- On `result`: A/B audition (target / seeded render / learned render), then
  **apply** (write values into `param` nodes, undoable) or apply per-param.
- **Round-trip verification before showing the final number**: render the
  learned params back through the actual eseq patch and score THAT against the
  target — not the trainer's internal render. (This is what the independent CPU
  metric already does on the dgen side; the host repeats it through its own
  render path.)
- `basin_check: wrong_neighborhood` renders as an honest "your patch is in the
  wrong neighborhood for this sample" state, offering the cold-restart result
  as an alternative instead of deltas.

## 9. Open questions

- Where the `dgenlisp` binary lives / how eseq locates it (env var? bundled?).
- Poly patches: v1 forces 1 voice; is that always well-defined for existing
  instruments?
- Params without `@min`/`@max`: refuse to learn, or apply default bounds?
  Leaning refuse-and-report in `plan` (explicit beats guessed search spaces).
- Stereo targets: v1 mono-sums the target? Trainer renders mono?
- Whether `--mode full` (basin search) ever makes sense seeded from a UI patch,
  given the v2 finding that init proximity did not predict refinement outcome.
