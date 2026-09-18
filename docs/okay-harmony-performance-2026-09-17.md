# okay-harmony performance

Saved scene **B13** (one-based scene 26), 120 BPM, 48 kHz, 512 frames,
four workers. The transport target of 32% is reached in the headless production
audio harness. The 130% Activity Monitor target **with the UI running is not
established**: the process measurements below exclude the UI.

## Whole-project results

| Metric | Original | Optimized |
|---|---:|---:|
| Mean callback budget | 42.01% | 31.02% |
| Median callback budget | 42.04% | 31.44% |
| p99 callback budget | 43.11% | 34.58% |
| Worst callback budget | 44.37% | 35.74% |
| Headless process CPU | 146.51% | 129.16% |
| Over-budget callbacks | 0 | 0 |
| New late/dropped events | 0 / 0 | 0 / 0 |
| Nonfinite audio blocks | 0 | 0 |

Each column contains two independently launched runs, each with 25 seconds of
musical warmup and 30 seconds of measurement: 5,626 measured blocks per column.
Callback percentiles use pooled blocks. Process CPU is the mean of the two
process measurements. All four helpers had verified audio workgroup membership.
Worker count, spin/wait policy, routing, parameter values, notes, and polyphony
were held constant. Both versions rendered nine Digi Drift voices, five PM Piano
voices, and the same remaining instrument voices.

The callback reduction is **26.16%**. Process CPU falls by **17.36 percentage
points**, or **11.85%**. Those savings do not justify claiming that the user's
180% full-app reading becomes 130%. With unchanged UI overhead, subtracting the
measured process saving would suggest roughly 163%; that is an estimate, not a
full-app measurement.

An initial baseline/candidate/candidate/baseline pass measured callback means of
42.04%, 30.96%, 30.96%, and 41.99%. After tightening the compiler's immutable
storage eligibility, the final compiler was rebuilt and measured twice again:
31.01% and 31.04%, with process CPU of 128.93% and 129.38%. The table uses those
final candidate runs. The final compiler produced the exact same offline audio
hash as the earlier candidate.

No builds or tests were run by this task during live measurement windows. The
desktop and its background processes remained running. These are measurements
of this saved scene on this Mac, not universal performance guarantees.

## Changes

- **PM Electric Bass:** put `event-hold` on coefficient inputs before expensive
  modal math. Previously only the finished coefficients were event-held, leaving
  their ancestors running at audio rate. The existing 16-sample cadence,
  immediate onset/note-off updates, 64 modes, and resonator recurrence remain.
- **SSL E channel:** recompute EQ coefficients when any smoothed EQ design input
  actually changes. Keep smoothing, mode crossfades, and filter state at audio
  rate. No threshold or slower automation rate was introduced.
- **Dust and Vulf:** share the linear cepstrum/lifter calculation at unit strength,
  then scale the complex log response for each old/new mix value. Exponentiation,
  impulse truncation, convolution, and the existing hop crossfade remain.
- **DGen C compiler:** support SIMD for independent scalar event expressions in
  complete groups of four active frames; use scalar execution for sparse groups
  and partial buffers, and skip inactive groups. Preserve adjacent expression
  fusion. Stop treating scalar lookups into initialized immutable tables as
  tensor chains, and lower their integer indices per SIMD lane. Mutable storage,
  uninitialized scratch, views, and feedback retain conservative scheduling.

Native 512-frame kernel timings isolate DSP work and exclude graph/UI overhead:

| Kernel/case | Original, µs | Optimized, µs | Reduction |
|---|---:|---:|---:|
| Bass, saved project settings | 682.2 | 133.0 | 80.5% |
| SSL, steady bus EQ (representative) | 691.3 | 149.5 | 78.4% |
| SSL, continuous EQ automation | 691.5 | 274.4 | 60.3% |
| Dust, steady | 615.6 | 481.4 | 21.8% |
| Vulf, steady | 708.9 | 564.2 | 20.4% |

Dust/Vulf automated cases also improved by about 20%. Timings use native thread
CPU time, alternating versions, warmup, and seven retained repetitions after
two discarded rounds. Earlier unsuccessful prototypes are retained separately
in the artifact directory and are not included in these results.

## Audio and correctness

The full-project offline comparison contains 1,440,256 stereo frames after
warmup. The difference has normalized RMS **0.00004832**, or **−86.32 dB relative
to the original signal**, and maximum absolute sample difference **0.00004128**.
RMS is 0.075943167 original and 0.075943134 optimized. This is numerical waveform
validation, not a listening test or a bit-identical claim across versions.

A final render loaded the unchanged project through its normal factory/user
asset names after removing the temporary comparison entries from the library.
It reproduced the optimized comparison's PCM hash exactly, with no nonfinite
blocks or new late/dropped events. `production-offline.result.json` records this
check; offline timing is not used in the live performance table.

Validation also includes:

- Bass's existing 192-render validation, independent modal reference, silence,
  pitch, release, retriggers, automation, and exact tested block partitions;
  plus the production `instrument_probe` compile/load/init path.
- Direct bass comparisons at 44.1/48/96 kHz, all presets, parameter extremes,
  automation, and generated-C fusion checks.
- SSL and both tone effects at 44.1/48/96 kHz, including adjacent/off-grid
  parameter changes, irregular process calls, and silent input. Differences in
  these effect comparisons are at float-rounding scale.
- 42 distinct focused DGen tests across the final validation passes: event
  scheduling, independent numerical table lookup references, immutable storage
  eligibility, mutable buffers, wavetable parsing, feedback/hop scheduling, and
  tensor scratch lifetime. No full workspace suite was run.

Both baseline and candidate log an existing native out-of-range parameter
mapping warning (`logical=1523`, `idx=3294967295`, `state_slots=152`). This is
tracked separately as **eseq-itgs**; it is distinct from the measured scheduler
late/dropped-event counters.

## Use and reproduce

The bass change is in the factory source. The three custom effect edits are in
`.local/effects/{ssl-e-channel-study,dust-comp,vulf-core-study}/dsp.lisp`.
The saved `okay-harmony.json` is unchanged. No sounds, presets, voice counts, or
project settings were replaced to obtain the improvement.

The compiler changes are published as
[DGenLisp v0.1.25](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.25),
source commit `d7a044ccdaa25fc49575a13f4ac4c551ecf6d6b5`. The macOS arm64
entry in `content/dgenlisp.lock` now selects this release, and
`scripts/fetch_dgenlisp.sh` has downloaded and verified its archive. The Linux
entry remains independently pinned to v0.1.20. Launch normally:

```sh
./target/release/metal_seq
```

No local compiler override is needed. Remove any older `ESEQ_DGENLISP_TOOL`
override from the launch environment to use the fetched compiler. Reload the
project to recreate its instruments/effects with the new compiler; an app
rebuild is unnecessary for the compiler pin. The compiler's six changed files
were committed and pushed for publication; the eseq edits remain in the working
tree. Implementation tasks `eseq-uitr` and `dgen-2fq`, and release task
`eseq-1nds`, are complete. Follow-up `eseq-d3pt` covers further
full-app CPU work. The user subsequently reported 31–33% transport and
151–160% Activity Monitor CPU in the running app.

The packaged executable is the tested release build, stripped and ad-hoc signed.
It passes standalone compilation with its bundled inline ABI/binary audit, and
the fetched default compiler emits byte-identical SSL C and passes the bass
host probe without compiler, header, audit, or toolchain overrides.
The full saved-project render through the fetched compiler also matches the
tested optimized PCM hash exactly, with zero nonfinite blocks or new
late/dropped events (`release-v0.1.25/verification.json`).
Release evidence is under the artifact directory's `release-v0.1.25/`.

Published archive SHA-256:
`91783065308bddf890cdedad329663fea0879d3382a3776e4b15982309f7f197`.
Packaged/fetched executable SHA-256:
`d37b6cb7d097616f80c7cf7e34b924510be07ac04948f5897da52f522102e8b8`.

Evidence and private project/source snapshots are under
`.local/benchmarks/okay-harmony-2026-09-17/`. `results.json` records the final
metrics and hashes; `live-*.result.json` contains individual blocks and voice
statistics. The `compare_*.py` files retain isolated numerical/timing drivers.
`audio_experiment` and `DGenLisp-candidate` are frozen measured executables.
Temporary library fixtures can be restored with `prepare_project_fixtures.py`
before rerunning `run_project_pairs.py`. Supply a fresh run tag to preserve the
original artifacts:

```sh
python3 .local/benchmarks/okay-harmony-2026-09-17/prepare_project_fixtures.py
OKAY_HARMONY_RUN_TAG=repeat1 python3 \
  .local/benchmarks/okay-harmony-2026-09-17/run_project_pairs.py final
python3 .local/benchmarks/okay-harmony-2026-09-17/prepare_project_fixtures.py --clean
```

Project SHA-256:
`43d9a365ee2928c3fc6f7f65a01185cde93149fde0ce6a276cefb4fe09dc0489`.
Final compiler SHA-256:
`4699a9771a96de482410b70cef11a87230edbd7dac994a61584e0dc7da8c180c`.
Frozen harness SHA-256:
`0b4553de19d0e0da5cfe9012a0524d17d4eda39b8834a11ff931fcec71620069`.
