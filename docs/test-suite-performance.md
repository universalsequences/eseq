# Test suite and build performance

## Current state (2026-11-13, Apple Silicon macOS, 10 cores, nextest 0.9.140)

Full workspace, `cargo nextest run --workspace --no-fail-fast`, 8,040 tests in
42 binaries (78 skipped):

| Configuration | Test phase | Cold test build | `target/debug` |
|---|---:|---:|---:|
| before: unoptimized dev profile | 46m15s | not measured cold | 54 GB |
| optimized dev profile | 5m50s | 6m09s | 8 GB |
| + preset-index walk fix | **3m41s** | same | same |

All three runs failed the same 20 tests; the profile change moved no test
between pass and fail. They are fixed, see the end of this section.

### What was wrong

- **Unoptimized code, not test count, was the cost.** 497 `host_kinds::tests`
  and 650 `state_values::tests` in the `metal_seq` binary each boot the real
  Lisp UI. Unoptimized, one boot is 6.5 s alone and ~42 s with ten running at
  once; those 497 tests were 20,970 of the run's 28,187 test-seconds. The same
  test is 1.3 s optimized.
- **The preset-name index walked the instrument tree once per instrument.**
  `refresh_preset_name_index` resolved every saved instrument without a
  leaf-walk snapshot, so each lookup re-walked its whole root. It runs on a
  background thread at every app start and every harness boot; kernel time per
  UI-boot test fell from 1.12 s to 0.43 s, and full-suite system time from
  1,410 s to about 450 s.

### What changed

- `Cargo.toml` `[profile.dev]`: `opt-level = 1`, line-table debuginfo;
  third-party dependencies at `opt-level = 2` with no debuginfo.
  `debug-assertions` and `overflow-checks` keep their dev defaults.
- `.config/nextest.toml`: tests are flagged at 30 s and killed at 2 minutes.
- `host_kinds::tests::arrangement_view::a_capture_in_flight_grows_the_rack_row`
  back-dated the record clock by 4 s, which saturates to zero in a process
  younger than 4 s. It passed only while booting the harness took longer than
  that; it now publishes the 8 beats directly.

### Edit loops

One-function edit, then rebuild. `fast` is the new incremental optimized
profile (`cargo run --profile fast -p sequencer --bin metal_seq`).

| Edit in | `--release` | `--profile fast` | test binaries (`--no-run`) |
|---|---:|---:|---:|
| `sequencer` library | 3m17s | 14 s | 17 s (lib or `metal_seq` tests), 30 s (workspace) |
| `metal_seq` binary (`src/ui`) | 1m22s | 3.5 s | 10 s |
| `eseqlisp` | 3m58s | 19 s | 10 s (eseqlisp tests), 44 s (workspace) |

`fast` cold-builds in 3m21s. Its runtime speed against `release` has not been
measured; it trades 16 codegen units for 256, so use `--release` for
benchmarks and shipping.

The first build of a new package selection is cold for the workspace crates
(about two minutes): `-p eseqlisp`, `-p sequencer` and `--workspace` resolve
different feature sets, so each keeps its own artifacts.

### Where the remaining time is

2,178 test-seconds in total: `metal_seq` binary 1,793 s (1,640 tests),
`sequencer` library 245 s (3,679), `eseqlisp` 119 s (2,706). The 1,147
UI-boot tests are 82% of it. The slowest test is
`graph_demos_view::every_graph_demo_loads_project_owned_and_rack_owned` at
22 s; nothing else exceeds 9 s.

### The failures, resolved (2026-11-14)

The 20 failures above were 10 once the other in-flight work was committed
(`c616dc47c`), and the suite is now green: 8,042 passed, 78 skipped.

- `ui/panel-header.lisp` used `eseq.materials/color-cell` without importing
  `eseq.materials` (5 tests; it only loaded when something else had imported
  the module first).
- Folded effect panels were keyed by the device's `did`, which is a
  placeholder until the device's first recorded edit. The first knob move on
  an effect therefore changed the key and re-ran the whole `*fx*` body
  mid-drag (`panels_view::editing_a_param_repaints_without_rebuilding_the_panels`).
  The fold state now holds the device handle.
- Three `piano_roll_view` tests carried lane-scroll constants for a 2-row
  lane footer (it is 1 row), one used `(track 0).steps`, which the reader
  does not parse, and its epoch baseline was sampled before the harness had
  settled.
- `metal_seq_browser_default_layout_has_one_relocated_search_input` still
  expected the search header beside the tab rail; `7deed0c4d` moved it above.

Seen once, not reproduced: `agent::tools::tests::effect_examples_are_named_by_folder_and_include_ui`
read a `__test-ambiguous-*` folder that a `lisp_host` test creates and removes
under the shared `.local/instruments`.

## Historical baseline (2026-08-20)

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
