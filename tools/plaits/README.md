# Plaits port tooling

Source and verification tooling for the `Synths/Plaits` factory instrument
(`content/instruments/Synths/Plaits`). That instrument's `dsp.lisp` is
**generated** from the files here; edit these, then regenerate.

```sh
EURORACK=/path/to/pichenettes/eurorack tools/plaits/regenerate.sh
```

regenerates the data tables in the instrument's `data/` folder (from
`plaits/resources.cc`, the LPC word banks and the DX7 sysex banks), the
generated engine files, and the assembled `dsp.lisp`. The port follows
eurorack commit `f4cfa769`; on that commit the output is byte-identical to
what is checked in.

## Layout

- `shell/header.lisp`: instrument inputs and params.
- `shell/voice.lisp`: `plaits/dsp/voice.cc`. Covers the 12-sample block clock, trigger and note handling, the decay envelope and its FM/TIMBRE/MORPH routing, and engine selection. Also covers the LPG and the output post-processing and limiter.
- `lib/common.lisp`: stmlib/plaits primitives shared by every engine.
- `engines/<name>.lisp`: one macro per engine,
  `(plaits-<name> bstart bphase rise high patched note timbre morph harmonics accent [extra…])`
  → `(tuple out aux)`. Some of these files are generated: `sixop`, `modal`, `additive` and `chordbank` come from `tools/gen_*.py`.
- `tools/assemble.py`: concatenates the header, common, the engines and the voice. It wraps each engine in `(block-gate (eq pidx k) …)`, so only the selected model runs.
  - The block-gate's whole upstream dependency cone must be scalar: no `delay`, and no delay time computed inside a gate.
  - Engines whose feedback loops need delay lines (string, particle, string machine) are marked `;@ungated`. Each one gates its own scalar parts and gets an `active` argument.

## Verifying against the original

`ref/plref.cc` renders one original engine or the full `Voice` to raw float frames. Build it with `EURORACK=… ref/build.sh`.

- `tools/plh.py` (`EngineTest`) builds a test instrument around one engine macro and compares it with the reference.
- `tools/voicecmp.py` compares the assembled instrument with the full `Voice`, including the module's 4-block trigger delay.
- `tools/test_<engine>.py` is the per-engine suite.
- `tools/plref_u` and `tools/plref_speech` build reference variants with deterministic noise. These are for the noise-driven engines.

Generated test instruments go to `$PLAITS_WORK` (default `~/.cache/eseq-plaits`).
