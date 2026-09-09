# Digi FM authoring and validation

`build.py` owns the routing table, spectral anchors, parameter declarations and
explicit 4× DSP integration. `build_ui.py` derives routing/spectrum graphics from
that data and combines them with the authored `ui-controls.lisp` and
`ui-body.lisp`. Generated factory files are checked in; regenerate with:

```sh
python3 tools/digi-fm/build.py
python3 tools/digi-fm/build_ui.py
```

The shared filter and envelope implementations live under `content/defmacros`.
The existing host materializes those imports; no external runtime files are
needed. Presets are authored independently in the adjacent factory preset bank.

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

Capture the factory panel through the real sequencer (macOS):

```sh
cargo run -p sequencer --bin metal_seq -- capture --script crates/sequencer/ui/capture-fixtures/digi-fm.lisp --buffer fx --track 0 --width 1800 --height 420 --out /tmp/digi-fm.png
```

The `digi-fm-a`, `-b`, `-harmonics`, `-phase`, `-filter`, and `-amp` fixtures
select the other center pages. See `docs/digi-fm-synth-design.md` for the source
manual, routing semantics, spectral recipe, measurements and limitations.
