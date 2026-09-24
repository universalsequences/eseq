# Digi FM authoring and validation

`build.py` owns the routing table, spectral anchors, parameter declarations,
8192-sample harmonic bank and explicit 4× DSP integration. `build_ui.py` derives routing/spectrum graphics from
that data and combines them with the authored `ui-controls.lisp` and
`ui-body.lisp`. Generated factory files are checked in; regenerate with:

```sh
python3 tools/digi-fm/build.py
python3 tools/digi-fm/build_ui.py
```

The shared filter and envelope implementations live under `content/defmacros`.
The host materializes those imports and loads the checked-in `spectra.json`
beside the factory DSP. Keep that asset with the instrument when copying it.
Presets are authored independently in the adjacent factory preset bank.

Each of the eight literal algorithms specializes a state-free four-substep
core behind `block-gate`. Phase, feedback and decimator histories remain shared
outside the gates, preserving live algorithm changes. The selected nonlinear
filter runs continuously; the other sleeps after their smoothed crossfade
reaches an endpoint. The table, algorithm dispatch and filter behavior match
the approved Digi FM Fast Test. Generation requires NumPy; runtime does not.

```sh
./scripts/fetch_dgenlisp.sh
./scripts/fetch_dgen_toolchain.sh
python3 tools/digi-fm/validate.py
cargo nextest run -p sequencer -E 'test(digi_fm_pages_expose_bound_visible_controls)'
cargo nextest run -p sequencer -E 'test(instrument_probe_applies_host_modulation_descriptors)'
cargo run -p sequencer --bin instrument_probe -- 'Synths/Digi FM' --preset 'Held Bell' --frames 8192 --min-peak .005 --min-rms .0001
```

The Python checks use NumPy and the existing raw audition harness. That harness
requires its binary audit script (`DGEN_BINARY_AUDIT_TOOL`); the validator honors
an override and otherwise uses the standard local dgen checkout path. It also
honors `ESEQ_DGENLISP_TOOL`. Compile artifacts stay in the audition cache, never
in factory content. The explicit import expansion in this test adapter is
separately covered by the production host probe.

For performance work, save the baseline DSP and its assets together before
editing, then run:

```sh
python3 tools/digi-fm/performance.py --baseline-source /absolute/path/baseline.lisp \
  --output /absolute/path/results
```

`--candidate-source` optionally selects an isolated experiment. The command
first compares all algorithms, factory presets, real host modulation inputs,
and regular/irregular process partitions at 32/128/512 frames. It then measures
the native process ABI in alternating baseline/candidate order, using the shared
C timing driver. Python, compilation and allocation are outside the timed
region. Results are single-voice CPU time, not the parallel application DSP
meter. Each source loads its own adjacent assets; both use the current shared
macros and pinned compiler. Sensitive
feedback can amplify rounding differences; a failed waveform comparison needs
investigation, not a looser threshold merely to accept a timing improvement.

See `docs/digi-fm-performance-2026-09-22.md` for the measured bottleneck,
optimization experiments and factory promotion evidence.

Capture the factory panel through the real sequencer (macOS):

```sh
cargo run -p sequencer --bin metal_seq -- capture --script crates/sequencer/ui/capture-fixtures/digi-fm.lisp --buffer fx --track 0 --width 1800 --height 420 --out /tmp/digi-fm.png
```

The `digi-fm-a`, `-b`, `-harmonics`, `-phase`, `-filter`, and `-amp` fixtures
select the other center pages. See `docs/digi-fm-synth-design.md` for the source
manual, routing semantics, spectral recipe, measurements and limitations.
