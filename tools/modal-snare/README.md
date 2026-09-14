# Modal Snare performance

The factory instrument caches its pure modal coefficients per voice. An exact,
audio-rate comparison detects changes to effective Stretch, Split, Tilt, Visc,
Batter Decay, Reso Decay, Tip and Bright values. First use and runtime sample-rate
changes also invalidate the cache. `event-hold` schedules the coefficient math;
explicit final `latch` nodes supply coefficients to the audio-rate resonators.

This retains the two 72-slot head banks, scalar head proxies, all 12 two-partial
wires, projection contact, rim, bend and pressure behavior. No modes, voices or
physical paths were removed, and no approximate math or control-rate clock was
introduced. Continuously modulating a cached input recalculates the coefficients
on every changed sample, so the static-preset saving is not guaranteed for that
workload.

The approach uses the same event scheduling support as the previous gamelan and
piano passes. Unlike their periodic coefficient clocks, this cache invalidates
on the exact sample an input changes.

## Reproduce

Keep a copy of the old source before editing. The September 14 baseline is
`.local/benchmarks/modal-snare-2026-09-14/baseline.lisp`, SHA256
`917148b1915b77123e31a0394fce100c229109b1bd8ef7ff453918d26e578e20`.
The report stores both source hashes and the compiler hash; source snapshots are
preserved under the specified output directory.

```sh
.local/venvs/physical-models/bin/python -B tools/modal-snare/performance.py \
  --baseline .local/benchmarks/modal-snare-2026-09-14/baseline.lisp \
  --compiler crates/sequencer/tools/DGenLisp-macos-arm64 \
  --params .local/benchmarks/modal-snare-2026-09-14/project-params.json \
  --output .local/benchmarks/modal-snare-2026-09-14/final
```

The Python environment needs NumPy and SoundFile. Fetch the pinned compiler and
toolchain with the repository scripts on a fresh checkout. Omit `--params` to use
the factory Jungle S settings. `--no-timing` runs only correctness comparisons.
Use a new output directory if either source changes.

The native benchmark reuses `tools/pm-gamelan/benchmark.c`: seven alternating
paired repetitions, four seconds of audio each, one voice at 48 kHz, velocity 1,
four notes, and 128/512-frame buffers. Compilation, allocation and Python are
outside the measured native process calls. Run without concurrent builds or
other benchmarks.

Validation covers all 15 presets plus the default at five pitches; all eight
cached controls changing on adjacent samples; runtime sample-rate changes;
audio-rate modulation at 44.1/48/96 kHz; and irregular process partitions with
gate, pitch and retrigger events. `check_fusion.py` checks each generated kernel.
Fixed-capacity channel buffers match the existing audition wrapper and host.

The host regression and probe use the app's normal compile/load/init path:

```sh
cargo nextest run -p sequencer --test modal_snare \
  -E 'test(=modal_coefficients_follow_parameter_events_without_delay) or test(=rim_pitch_tracks_played_notes)'
cargo run -p sequencer --bin instrument_probe -- 'factory:Drums/Modal Snare' \
  --preset 'Jungle S' --sample-rate 48000 --frames 96000 --midi-note 43 \
  --min-peak 0.01 --min-rms 0.001
```

Raw local project configurations and runs are under
`.local/benchmarks/modal-snare-2026-09-14`. They use an untouched snapshot of
`modalsnaretest`, with only the instrument identifier redirected to temporary
baseline/candidate copies. Build the host driver with
`cargo build --release -p sequencer --features audio-experiments --bin audio_experiment`,
then run `target/release/audio_experiment CONFIG.json`. The project comparison
uses scene index 1, four workers, 48 kHz, eight seconds of warmup and sixteen
seconds of measurement, in baseline/candidate/candidate/baseline order. The
temporary instruments can be recreated from the saved source snapshots under
`.local/instruments/modal-snare-benchmark-a5v5/{baseline,candidate}/dsp.lisp`.

See [the measured report](../../docs/modal-snare-performance-2026-09-14.md) and
[raw validation and timing results](performance.json).
