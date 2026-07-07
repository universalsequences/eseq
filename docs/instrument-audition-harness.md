# Instrument Audition Harness

`tools/audition/audition.py` compiles a folder custom instrument's `dsp.lisp`
outside the app and drives the **real compiled DSP** from Python, so an
instrument can be measured (spectra, decay times, stability) or rendered to
WAV without launching the sequencer. This method has been the backbone of
every physical-model instrument built here (membrane-snare, membrane-tabla,
the ultrakick/snare/perc family, operator, wavetable): every tuning decision
is made against measurements of the actual dylib, never against a Python
model of the DSP.

## How it works

1. **Preamble + compile.** Instruments don't compile standalone — the app
   injects a shared helper preamble (`INSTRUMENT_PREAMBLE` in
   `crates/sequencer/src/lisp_host.rs`: svf, ladder, adsr, polyblep
   oscillators, …). The harness extracts that constant, concatenates it with
   the instrument's `dsp.lisp`, and compiles via `swift run DGenLisp` in the
   dgen repo (default `~/code/swift/dgen`, override with `DGEN_ROOT`). The
   instrument folder is passed as `--asset-base` so tensor `@default-file`
   JSONs resolve.
2. **Manifest.** Compilation produces `patch.dylib` + `patch.json`. The
   manifest is the contract: `params[].cellId` (params are set by writing
   floats directly into the state array — there is no setter call),
   `tensorInitData[].offset` (tensor contents to load into state before the
   first block), `inputs`/`outputs` channel maps, `totalMemorySlots`, and
   `processAbi`.
3. **Drive.** ctypes loads the dylib and calls
   `process(float** in, float** out, int nframes, void* state, void* buffers, float sampleRate)`
   (ABI `dgen-c-v2-host-sample-rate`; `buffers` is NULL) in `max-frames`
   blocks. Inputs follow the instrument contract: gate / pitch(Hz) /
   velocity / trigger / clock on channels 0–4, mod1–4 on 5–8.
4. **Measure.** numpy FFT on the output: partial ratios, T60 decay,
   NaN/limit-cycle checks.

Builds are cached in `~/.cache/eseq-audition/<hash>` keyed on the preamble,
the dsp.lisp, **and the contents of sibling .json/.wav assets** — tensor
default-files are baked into the dylib at compile time, so editing a mask
JSON correctly triggers a recompile. First compile takes ~a minute (swift
build); cached runs are instant.

## CLI

From the repo root:

```sh
# quick report: peak, NaN check, T60, partial ratios
python3 tools/audition/audition.py crates/sequencer/instruments/drums/membrane-tabla

# see the instrument's params and ranges
python3 tools/audition/audition.py <instrument> --list-params

# render a specific articulation to WAV
python3 tools/audition/audition.py crates/sequencer/instruments/drums/membrane-tabla \
    --pitch a2 --set stroke=1.0 --seconds 2 --wav /tmp/na.wav

# p-lock style automation: bayan press gliss (ramp press 0->1 over 0.2..0.8s)
python3 tools/audition/audition.py crates/sequencer/instruments/drums/membrane-tabla \
    --pitch g2 --set gliss_range=0.5 --ramp "press=0.2:0,0.8:1" --wav /tmp/ga.wav

# melodic instruments: note names, gate release, retriggers
python3 tools/audition/audition.py crates/sequencer/instruments/core/drift \
    --pitch c#3 --gate-off 0.6 --retrig 0.3,0.6
```

Flags: `--seconds --pitch --vel --set NAME=V --ramp NAME=T:V,T:V --retrig t,t
--gate-off t --wav path --sr --max-frames --list-params -v`. `--ramp` targets
either a param (a p-lock) or an input channel (`pitch`, `mod1`, …).

## Library use (for tuning sessions)

The per-instrument experiments — mode-shape imaging, stroke-morph sweeps,
per-partial decay — are written as short scripts on top of the module:

```python
import sys; sys.path.insert(0, "tools/audition")
from audition import Instrument, partials, t60, report, write_wav

inst = Instrument("crates/sequencer/instruments/drums/membrane-tabla")
inst.manifest          # full compile manifest (cellIds, tensor offsets, io)

# render() returns (audio, state_memory) — keep mem to inspect internal state
y, mem = inst.render(2.0, pitch=220.0, params={"stroke": 0.5, "syahi": 1.0})
report("tun", y)                       # peak / nan / T60 / partial table
pk = partials(y, sr=inst.sample_rate)  # [(amplitude, hz)] sorted by freq

# sweep a param and stack measurements
for s in [0.0, 0.5, 1.0, 1.5]:
    y, _ = inst.render(2.0, params={"syahi": s})
    report(f"syahi={s}", y)

# expression automation, retriggers, stability torture
y, _ = inst.render(4.0, retrig=[0.3, 0.6, 0.9],
                   params={"syahi": 1.5, "press": 1.0},
                   ramps={"press": [(0.2, 0.0), (0.8, 1.0)]})
```

Reading internal DSP state: `mem` is the live state array. Tensor offsets
from `inst.manifest["tensorInitData"]` / `["tensors"]` locate things like an
FDTD head's displacement grid — recording a grid slice every block and
FFT-ing per cell yields measured mode shapes (this is how the tabla's
loaded-head nodal lines were found).

## Standard verification checklist for a new instrument

- **Silence check**: default render, `peak > 0`, `nan=False`.
- **Pitch**: f0 tracks the pitch input; check `tune` default puts the sung
  pitch on the host note.
- **Spectrum**: partial ratios where you want them (e.g. tabla's Raman series
  1.00 : 1.99 : 2.74 at defaults); overtones not buried (broad strike/read
  masks act as spatial lowpasses — see the membrane instruments' comments).
- **Every expressive param sweeps min→max** without blowup — these will be
  p-locked.
- **Stability torture**: several seconds, retrigs mid-decay, all extreme
  params at once; the tail must decay toward 0 (no NaN, no limit cycle).
- **Velocity** scales sensibly (physical strikers: brighter, not just louder).

## Gotchas learned the hard way

- **All param defaults must be written into state** before the first block
  (`Instrument.fresh_memory` does this). Raw zeros = nonsense state and
  usually silence; this once cost hours of misdirected DSP debugging.
- **Stale dylibs keep old tensor data** — tensor JSONs are baked in at
  compile time. The cache key accounts for this; if bypassing the harness,
  recompile after editing any mask JSON.
- **Don't compare measurements across builds of a suspect compiler.** Buffer
  allocation differs per compile, so a compiler bug can move between builds;
  compare variants only within one build, and use a known-good instrument
  through the same harness as a control.
- Block size doesn't affect output (verified 64–512), so `--max-frames 128`
  is a fine default and compiles faster than 512.
- `partials()` on a signal with a pitch ramp smears — measure gliss by
  windowed FFTs early vs late instead.
