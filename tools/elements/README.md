# Elements port tooling

Source and verification tooling for the `Synths/Elements` factory instrument
(`content/instruments/Synths/Elements`). Its `dsp.lisp` is **generated** from
the files here; edit these, then regenerate.

```sh
EURORACK=/path/to/pichenettes/eurorack tools/elements/regenerate.sh
```

regenerates the data tables in the instrument's `data/` folder (from
`elements/resources.cc`: lookup tables, the strike samples and the blow noise
recording), the generated modules, and the assembled `dsp.lisp`. The port
follows eurorack commit `f4cfa769`; on that commit the output is
byte-identical to what is checked in.

## Layout

- `shell/banner.lisp`, `shell/header.lisp`, `shell/footer.lisp`: the instrument's comment banner, its host inputs and params, and its outputs.
- `shell/test_header.lisp`, `shell/test_footer.lisp`: the reference-comparison driver. It takes render-controlled params and a deterministic noise option, and outputs the reference's debug channels.
- `src/lib.lisp`: stmlib primitives, the 0.5 ms block clock and the host-rate rules (`kr`, `blk`, `tb`).
- `src/envelope.lisp`, `src/exciter.lisp`, `src/string.lisp`, `src/reverb.lisp`, `src/voice.lisp`: the module's `multistage_envelope`, exciters + tube + diffuser, string, fx/reverb, and voice + part.
- `src/resonator.lisp` (from `tools/gen_resonator.py`) and `src/ominous.lisp` (from `tools/gen_ominous.py`) are generated.
- `tools/assemble.py`: concatenates the shell and modules (`--test` builds the driver).

## Verifying against the original

`ref/elref.cc` drives `elements::Part` at 32 kHz in 16-sample blocks. Build it (and `ref/elbench`) with `EURORACK=… ref/build.sh`.

- With `ELREF_DET=1`, every random draw comes from a counter sequence. The driver reproduces it with `det=1` and the per-block draw layout (`det_d`, `det_str`).
- `tools/elh.py` renders driver and reference side by side.
- `tools/test_modal.py`, `test_string.py` and `test_part.py` are sample-level suites.
- `tools/test_stats.py` compares the noise-driven paths statistically.
- `tools/srcmp.py` compares the instrument at a host rate with the resampled 32 kHz module.
- `tools/robust.py` sweeps extreme settings at 44.1 and 96 kHz.
- `tools/test_switch.py` switches between every pair of models while playing (the models are gated, and a gate opens mid-block), and flags output that goes non-finite or sticks at one value.
- `tools/presets.py` writes `Elements.presets`, loudness-normalized.
- `tools/bench.py` measures CPU against the original: `ref/elbench` (the C++ `Part` at -O3, built by `ref/build.sh`) and `tools/dgdrv.c` (the compiled instrument) play the same note pattern, and both are timed by the CPU's cycle counter, per scenario. On an M1 Max, one voice costs 2.1 % of a core in the modal model at 32 kHz (3.9x the C++, 3.0 % at 48 kHz), 0.9 % for one string, 1.8 % for five strings and 3.5 % for Ominous.
- `tools/cpuparts.py` measures the modules on their own.

Set `ELEMENTS_DATA` to use a data folder other than the instrument's. Generated test builds go to `$ELEMENTS_WORK` (default `~/.cache/eseq-elements`).
