# Physical-model coefficient scheduling

Measured on Apple M1 Max, macOS, 48 kHz. Baseline: scheduler-fix commit
`290777a5`, DGenLisp v0.1.19. These are further gains beyond the earlier
constant-tensor compiler fix and previous modal-bank reductions.

| Instrument | Default 128 frames, before → after (µs) | Default 512-frame speedup | Range across cases |
| --- | ---: | ---: | ---: |
| PM Bonang | 104.2 → 55.3 | 2.09× | 1.87–2.17× |
| PM Crash | 79.3 → 57.8 | 1.35× | 1.34–1.37× |
| PM Hi-Hat | 95.3 → 70.4 | 1.33× | 1.33–1.37× |
| PM Kempyang | 40.3 → 22.6 | 1.73× | 1.73–1.79× |
| PM Kethuk | 72.1 → 28.5 | 2.51× | 2.51–2.72× |
| PM Ride | 79.3 → 57.4 | 1.36× | 1.36–1.40× |
| PM Saron | 70.0 → 37.0 | 2.23× | 1.89–2.36× |
| PM Slenthem | 46.4 → 24.7 | 1.89× | 1.87–1.89× |
| PM Slenthem Slendro | 102.4 → 48.8 | 2.25× | 2.06–2.27× |

The six gamelan models gain 1.7–2.7×. Cymbals gain 1.3–1.4×; this work
does **not** achieve 2× for every physical model. Piano, cello, clarinet,
flute, and saxophone retain their current implementations.

## What changed

Gamelan coefficient inputs already latched on strikes and every 16 samples,
but their derived table interpolation, exponential and trigonometric work
still ran every sample. The new `event-hold` compiler boundary schedules
those pure calculations on the existing update clock. Ordinary final latches
feed continuously running resonators. An onset still updates coefficients
on that exact sample, including between periodic ticks.

Cymbals use the same scheduling boundary. All six physical regions, eighteen
delay paths, eighteen radiation filters, eight resolved modes, and hi-hat
collision dynamics remain. The delay paths now share one tensor-bank loop
and write cursor; each lane retains its own delay and allpass state.

Contact and stereo coefficients in gamelan, and touch coefficients in cymbals,
now update on that same strike/16-sample clock. Their state and smoothers
remain at audio rate. This adds at most 15 samples of coefficient latency
(0.313 ms at 48 kHz) during automation. No modes were pruned, decay tails
shortened, or recordings substituted for synthesis.

The compiler also preserves one sample loop across mixed-rate feedback
fragments. Its renderer and memory allocator share those scheduled regions,
preventing scratch-buffer reuse between interleaved fragments. C forward
rendering is supported; Metal and autodiff explicitly reject `event-hold`.

## Validation

- Targeted Swift checks cover event timing, adjacent events, dense event
  storage, mixed clocks, feedback, buffer reuse, block gates and periodic hops.
- Five gamelan families: 1,245 signal/control checks, 87 tuning checks across
  44.1/48/96 kHz, and all existing reference-audio regression thresholds.
- Saron: 272 checks, exact onset-phase independence, calibration-table and
  patch-editor writeback checks, plus all 35 reference recordings passing
  attack, level, modal and upper-mode comparison gates.
- Cymbals: 186 signal/control checks, close/reopen contact behavior, irregular
  block partitioning, preset and sample-rate checks.
- Three exact `pm_woodwinds` sidecar tests; compiled editor writeback audio
  for all nine models. Cymbal writeback maximum error was below 2.7e-7.
- All nine load and render through `instrument_probe`, using the host compiler,
  initializer and production process ABI.
- Published DGenLisp v0.1.20 distributions compile all nine models with their
  bundled runtime resources on macOS arm64 and Linux x86_64. Standalone checks
  cover adjacent triggers, finite audio/state, onset silence, and 512-frame,
  one-frame and irregular partitions; partition errors are zero on both targets.
  Linux execution used QEMU 8.2.2, so these are functional checks, not native
  Linux performance measurements. Provenance and hashes are recorded in
  [distribution-check.json](distribution-check.json).

Paired native timing runs alternate baseline/candidate order seven times at
128 and 512 frames. Cases cover MIDI 48/60/84, velocities 0.4/0.8/1, and a
factory preset as well as defaults. The C driver retriggers exactly twice
per second regardless of block size. Compilation, Python, allocation and I/O
are outside the timed region. These are single-voice DSP costs, not projected
transport or Activity Monitor savings.

Phase-aligned audio comparisons cover every factory preset at three pitches
and velocities. Maximum normalized waveform RMS difference was below 1e-5
for gamelan/Saron and 5.3e-4 for cymbals. These numbers describe waveform
error, **not perceptual similarity percentages**. Parameter automation changes
are covered by the family tests; the waveform comparison uses static presets.

Raw paired timings and audio metrics are in [performance.json](performance.json).
Generated audition audio stays under `.local/benchmarks/physical-models-2026-09-11/final/`.

## Reproduce

Create a baseline directory using the physical-model sources from `290777a5`
and unpack the v0.1.19 compiler distribution separately. Compile the candidate
with the current published compiler. In the physical-model Python environment:

```sh
python tools/physical-models/performance.py \
  --baseline /absolute/baseline-models \
  --candidate /absolute/candidate-models \
  --baseline-compiler /absolute/v0.1.19/DGenLisp \
  --compiler /absolute/current/DGenLisp \
  --output /absolute/results
```

Candidate directories include each model’s `.presets` bank. For source changes,
build in a fresh staging directory without old authored sidecars, then run:

```sh
cargo run -p eseqlisp --example pm_factory_sidecars -- /absolute/staging
ESEQ_PM_FACTORY_DIR=/absolute/staging ESEQ_PM_VERIFY_DIR=/absolute/writeback \
  cargo nextest run -p eseqlisp --test pm_woodwinds \
  -E 'test(=factory_saron_sidecar_preserves_executable_controls) or test(=factory_gamelan_sidecars_preserve_executable_controls) or test(=factory_cymbal_sidecars_preserve_executable_controls)'
```

Run each family’s verification against that directory and compiled writeback
before installation. `ESEQ_DGENLISP_TOOL` selects the candidate compiler.
No broad workspace test suite is needed for this workflow.
