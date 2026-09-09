# FM/formant feasibility experiment

Research evidence for [the investigation](../../docs/fs1r-factory-synth-investigation.md),
2026-09-08, Bead `eseq-hhjb`. These files are **not factory content**.

`probe.lisp` implements a Gaussian phase-aligned formant (PAF) candidate with
optional phase modulation of its two carrier cosines. Its three outputs are
audio, base phase, and modulation phase. It is based on the published
[PAF construction](https://msp.ucsd.edu/techniques/v0.07/book-html/node88.html),
not a reconstruction of Yamaha's DSP.

`measure.py` uses the existing audition harness with an explicitly selected
compiler. It checks eight static cases against a NumPy reference using the
compiled phase outputs, compares 64/128/256-frame rendering, builds an
eight-pair workload, and measures process-call time. It also compares analytical
48 kHz signals against ideal lowpass/downsampled 8x and 16x references to expose
aliasing. NumPy is its only additional Python dependency.

Run from the repository root on macOS:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tools/fs1r-research/measure.py \
  --compiler "$PWD/crates/sequencer/tools/DGenLisp-macos-arm64" \
  --toolchain-root "$PWD/crates/sequencer/tools/dgen-toolchain" \
  --audit-tool "$HOME/code/swift/dgen/scripts/audit-dgen-dylib.sh" \
  --output-dir /tmp/eseq-fs1r-research-new
```

Adjust the audit-script path to your DGen checkout. The published compiler used
here lacked an executable-relative audit script; the actual host performs its
own audit, whereas the standalone harness needs this explicit path. The
experiment does not bypass the audit. The script and existing ctypes loader
are macOS-only; no Linux results are claimed.

The output directory receives `probe.lisp`, generated `bank.lisp`, and
`results.json`. Use a fresh directory to preserve earlier runs. Compiled files
remain in the audition harness's normal cache. The script checks each compiled
C file with `tools/audition/check_fusion.py`; that check covers one known bug
pattern, not general compiler correctness.

The production host compile/load/render smoke check is:

```sh
target/debug/instrument_probe /tmp/eseq-fs1r-research-new/bank.lisp \
  --frames 48000 --sample-rate 48000 --gate-frames 24000 \
  --min-peak 0.01 --min-rms 0.001 --json
```

This investigation used the existing built `instrument_probe`, without a Rust
rebuild. Rebuild that target with `cargo build -p sequencer --bin instrument_probe`
when validating subsequent host changes.

## Baseline evidence

- `baseline/results.json`: rerun using the checked-in measurement script;
  compiler and input hashes, numerical results, all timing batches.
- `baseline/bank.lisp`: generated workload corresponding to that result.
- `baseline/host-probe.json`: host smoke check, A4, 48 kHz, one second.
- `baseline/initial-results.json`: exploratory run before making the script
  relocatable. Preserved because its timings were much noisier; it is not an
  independent benchmark or a different implementation.

## Limits

- Width is a **dimensionless shaping index**, not a calibrated bandwidth in Hz.
  Skirt is not independently implemented.
- Static controls only. No safe general transition policy, production
  feedback model, anti-aliasing, motion sequencer, or preset system is present.
- Numerical agreement uses the compiled phase diagnostics. It verifies the
  waveform arithmetic, not independent phase-accumulator accuracy.
- Alias figures come from analytical float64 signals, not from DGen recordings.
  They are relative RMS errors, not dBFS or a listening score.
- The bank shares one fundamental phase, has eight amplitude envelopes and
  eight noise bandpasses, and feeds voiced outputs through a fixed PM chain.
  Each pair shares its envelope between voiced and noise output. It is smaller
  than a full instrument with independent frequency/noise envelopes.
- Timing includes Python/ctypes call overhead and excludes the audio graph,
  modulation host, polyphonic scheduling, effects, and anti-aliasing. No claim
  about shipping polyphony, final CPU, perceptual quality, or FS1R equivalence.
