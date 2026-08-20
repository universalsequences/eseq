# Test suite performance baseline

Measured for bead `eseq-4tl` on 2026-08-20, Apple Silicon macOS, with
`cargo-nextest 0.9.140`. Timings below are warm-build test times unless a build
time is called out. Commands use nextest's default concurrency.

## Commands

```sh
cargo nextest run --workspace --no-fail-fast \
  --status-level all --final-status-level all

cargo nextest run --workspace --release --no-fail-fast \
  --status-level all --final-status-level all

# Per-crate debug wall time
/usr/bin/time -p cargo nextest run -p <crate> --no-fail-fast
```

`.cargo/config.toml` supplies the required 16 MiB test stack automatically.

## Result

| Profile | Tests | Skipped | Test wall time | Process wall time |
|---|---:|---:|---:|---:|
| debug | 4,272 passed | 32 | 191.09 s | 192.06 s |
| release | 4,270 passed | 32 | 48.13 s | 48.90 s |

The first release validation rebuilt the profile in 4m25s and then ran the tests
in 48.79s. The table uses the immediate warm rerun so compilation is not
mistaken for suite execution time.

The previous tuning record was 25 minutes down to 105 seconds for 3,310 passing
tests (2026-07-28). The current debug suite has grown by 962 tests (29%) and is
now 191 seconds. This is still a real regression after accounting for growth:
process-isolated `metal_seq` tests repeatedly compile/load the large Lisp UI,
and two 16-node graph UI tests dominate the remaining tail.

A clean-HEAD diagnostic run found 252 debug failures (mostly stack aborts) and
87 release failures. Its wall times are not useful speed baselines: aborting
made debug tests finish early, and the isolated release worktree lacked the
staged gitignored DGen toolchain. The documented clean-HEAD command in
`CLAUDE.md` and `AGENTS.md` now reuses the primary checkout's staged toolchain.

## Per-crate debug test wall time

| Crate | Tests | Skipped | Test wall time |
|---|---:|---:|---:|
| `musicplayer` | 0 | 0 | 0.00 s |
| `eseqlisp` | 1,657 | 6 | 22.47 s |
| `sequencer` | 2,615 | 26 | 158.21 s |

The first `musicplayer` command rebuilt a different dependency feature graph in
36.73s; it contains no tests, so that build is excluded from suite timing.

## Test-binary cost (debug)

These are aggregate per-test seconds, not wall time; nextest runs tests in
parallel. They rank where work is concentrated and include each target's
slowest individual test.

| Target | Tests | Aggregate test-seconds | Slowest |
|---|---:|---:|---:|
| `sequencer::bin/metal_seq` | 695 | 933.1 s | 7.40 s |
| `sequencer` library | 1,918 | 747.1 s | 28.59 s |
| `eseqlisp` library | 1,654 | 222.2 s | 7.62 s |
| other integration/binary targets | 6 | 0.6 s | 0.33 s |

## Slowest debug tests after tuning

| Seconds | Test |
|---:|---|
| 28.59 | `lisp_host::tests::graph_16_demo_ui_exposes_all_node_controls_and_ring_defaults` |
| 26.35 | `lisp_host::tests::graph_16_cycle_demo_round_trips_resolution_and_quantize_cycles` |
| 10.01 | `effects::multiverb::tests::tail_survives_long_silence_without_denormal_blowup_and_stays_quiet` |
| 9.70 | `effects::filter_table_presets::tests::factory_presets_bake_deterministically` |
| 7.94 | `effects::multiverb::tests::all_modes_and_extreme_params_stay_finite` |
| 7.86 | `effects::multiverb::tests::longer_decay_setting_holds_more_late_energy` |
| 7.68 | `lisp_host::tests::graph_8x8_demo_ui_exposes_node_param_controls_and_weight_matrix` |
| 7.62 | `live_spectrogram_waterfall_renders_narrow_high_frequency_energy` |

## Tail changes in this pass

The complete debug run before tail work took 248.77s (247.7s test phase). After
the changes below it took 192.06s (191.09s test phase), a 22.8% wall-time
reduction.

- Removed one 22-instrument layout sweep that took 66.06s under suite load
  (39.27s alone). Repository policy explicitly excludes per-instrument layout
  suites for experimental instruments; curated shipping instruments retain
  their focused layout tests.
- Kept all deterministic patcher writeback fuzz seeds, but reduced external
  DGen compiler launches from 88 to 24 representative samples. The three tests
  fell from 14.9–19.2s each to about 3.1s each in an isolated diagnostic run.
- Reduced the multiverb denormal-tail render from 30s to 10s per mode. Ten
  seconds is still over twenty configured decay times; the test fell from
  28.8s to 5.5s in isolation.
- Moved `causal_cost_attribution` out of the correctness path with an explicit
  `#[ignore = "eseq-4tl: ..."]`; it was a print-only best-of-three benchmark
  with no assertions and cost 12.6s.

The next structural speed target is shared/serialized compilation of the two
16-node graph UI fixtures. Their behavior coverage is valid, so this pass did
not weaken or remove it merely to improve the number.
