---
name: identify-drum
description: Identify a sampled drum hit (R-8, 808, 909, LinnDrum, Virus…) as a compact scalar synth patch by SynthID-style optimisation and ship it as an eseq factory instrument (dgenlisp dsp.lisp + ui.lisp + presets + layout test). Use when the user names a sample in the eseq sample library and wants "the perfect <sound>" recreated, not sampled.
---

# Identify a sampled drum → eseq instrument

The process that produced `content/instruments/Drums/808 Clap` (R-8 MkII '808Clap',
noise voice) and `Drums/Virus B BassDrum 23` (swept kick with a harmonic ladder,
2026-09-03, second attempt after a sine-sweep voice scored a fine gate number and
missed the sound). Two repos: the fit lives in the dgen checkout
(`~/code/swift/dgen`, `Examples/SynthID/`), the instrument in eseq. Budget: ~1 hour of
measurement + topology, then 2–5 minute fit rounds; expect 4–8 rounds.

The contract (from `Examples/SynthID/SPEC.md`, keep it): parameters are ordinary
scalars with documented bounds; no waveform tables, residual tables or target-derived
arrays ever become parameters; the acceptance number is the independent metric
`scripts/compare.py` against a deterministic midpoint baseline; the eseq port must
reproduce the fit's `learned.wav`.

**The gate is necessary, not sufficient.** It is dominated by whatever is loud, has a
-60 dBFS floor, and penalises noise it cannot phase-match. It cannot see a harmonic
ladder 25–50 dB under the fundamental, and it drives recording hiss to zero. Every
round needs the diagnostics in §3 *and* the user's ear on `ab.wav` before "done".

## 0. Locate the sample

Samples are stored by hash. Titles live in `samples.jsonl`, not the sqlite:

```bash
grep -i '"<name>' .local/samples.jsonl            # dev layout
grep -i '"<name>' "$HOME/Library/Application Support/com.universalsequences.eseq/samples.jsonl"
# -> {"hash":"<sha256>","title":"...","tags":[...]}
cp .local/samples/<sha256>.wav ~/code/swift/dgen/Assets/<machine>-<sound>.wav
```

Record provenance (tags name the machine). Duplicate titles at two sample rates are
usually one hit resampled: check correlation before picking one. Note the native rate:
R-8 files are 26,040 Hz (13 kHz band-limit), the Virus B is 32.5 kHz (16.25 kHz) — that
becomes a fixed output stage later, never a fitted parameter.

## 1. Measure before writing any DSP

Run from the dgen root and write the numbers into the handoff doc:

```bash
python3 Examples/SynthID/scripts/analysis/analyze_harmonic_ladder.py Assets/<file>.wav   # pitched
cp Examples/SynthID/scripts/analysis/analyze_808_clap.py analyze_<sound>.py               # noise-driven
```

What to read off (both scripts print it):

- housekeeping: sr, length, peak dBFS, DC, channels identical, −60 dB end, noise floor
- fine envelope (0.5 ms RMS): burst/transient onsets and spacings, per-burst decay
- tail decay per window (log-RMS slope): one stage or two? T60?
- band spectra of attack vs tail vs late (1/3-octave), spectral tilt
- **pitched sounds — the harmonic ladder** (`analyze_harmonic_ladder.py`):
  - f0(t) from positive zero crossings, and the rms log error of one- vs
    two-exponential pitch models against it. One exponential missed the Virus sweep by
    30–40 % from 30 to 70 ms; the gate never noticed. Use two if the error halves.
  - each harmonic's level relative to H1 per window, heterodyned along the measured
    phase with a **two-period** Hann window. A fixed short window leaks between
    neighbours once f0 is low and fakes a flat "pulse" spectrum; a long FFT shows the
    fundamental's Hann sidelobes as fake ridges. Both fooled a round.
  - the slope of Hk−H1 over the tail: ≈0 or ≈H1's own slope means oscillator content
    (additive bank: one level + one extra decay per harmonic); ≈(k−1)×H1's slope means a
    static waveshaper. Do not add a tanh and hope — the Virus's odd harmonics were not
    saturation products, and a tanh that the fitter could not turn off manufactured a
    -30 dB H3 the sample lacked.
  - the residual after resynthesising harmonics 1..20, band-tabled: 20+ dB down below
    1 kHz means the sound *is* harmonics; what remains is transient or texture.
  - **recording texture**: >4 kHz RMS per 25 ms and its 1/3-octave shape. A hiss at
    -70 dBFS that decays with the hit and stops at the machine's Nyquist is part of the
    sound's character ("finger on a guitar cable, high-passed" was the user's words).
    It sits ~-90 dB *per bin*, under the gate's floor — it must be measured here.
  - the first 60 samples: does the hit open with an impulse or grow in?

Decide the family:

| family | voice path | reference |
|---|---|---|
| pitched swept (kick, tom) with a harmonic ladder | NumPy additive bank + CPU fit, `scripts/fit_virus_kick.py` as template | `HANDOFF_ACCESS_VIRUS_B_BASSDRUM23_REDO.md` |
| pitched, genuinely sine-like (check the ladder first!) | Swift SynthID profile, GPU autograd + `refine_rung3.py` | `HANDOFF_909.md`, `HANDOFF_808_TOM.md` |
| noise-driven (clap, snare noise, shaker, rim) | NumPy voice + CPU fit, `scripts/fit_clap.py` as template | `HANDOFF_808_CLAP.md` |
| noise + metal ring (hat, cymbal, ride: narrow peaks in the tail) | NumPy noise wash + **struck-once decaying-sine mode bank**, `scripts/fit_hat.py` as template | `HANDOFF_909_OPEN_HAT.md` |
| mixed (snare = pitched body + noise) | NumPy voice with both blocks; swept-phase body from `fit_virus_kick.py` | both |

The NumPy path is preferred whenever it can express the voice: no autograd, no
fdcheck, every topology change is a one-line edit, and a round is 1–2 minutes.

## 2. Design the voice (smallest scalar set the measurements demand)

Rules that held: every capacity addition is zero-default and inert; closed-form
envelopes `exp(rate·(t−t0))·[t≥t0]` on a time ramp, never stateful envelopes;
frequencies and decay rates in log / logneg space; never initialise on a bound; clamp a
seed into any bound you tighten (a seed outside the bound is otherwise silently kept).
Fixed machine properties (output band-limit, output stage) are constants in the voice,
not fitted params. Onset times of a flam are individual scalars, not one spacing.

Pitched voices (`fit_virus_kick.py::render`):

- phase `φ = fEnd·t + a1/r1(e^{r1 t}−1) + a2/r2(e^{r2 t}−1)`, wrapped (`φ − floor φ`)
  before every sine so the float32 port stays exact
- bank `Σ h_k e^{d_k t} sin(2π k frac φ)`, k = 1..10 (h_1 = bodyAmp, d_1 = 0)
- saturator **gain-normalised**: `tanh(d·x)/d · outGain`. Without this, coordinate
  descent cannot lower `drive` because doing so alone also lowers the level.
- one lowpassed noise burst for the onset, one high-passed hiss with its own slow decay
  for recording texture, a decaying-sine click capped small (the Virus had none; an
  uncapped click became a 0.095 impulse on sample one)

Everything must be expressible in dgenlisp with: `(noise)` (xorshift, identical to
`render_reference.dgen_noise`), `biquad`, `accum`/history ramp, `exp`, `pow`, `floor`,
`sin`, `tanh`, `gswitch`, `lt/gt`.

## 3. Fit

```bash
cd ~/code/swift/dgen
cp Examples/SynthID/scripts/fit_virus_kick.py Examples/SynthID/scripts/fit_<sound>.py   # or fit_clap.py
# edit: BOUNDS, MEASURED (from step 1), render(), fixed machine constants
python3 Examples/SynthID/scripts/fit_<sound>.py --target Assets/<file>.wav \
   --out output/<sound>_v1 --restarts 30 --keep 5 --passes 8 --steps 15 --final-passes 6
python3 Examples/SynthID/scripts/analysis/deficit_table.py output/<sound>_v1 \
   --fit-module Examples/SynthID/scripts/fit_<sound>.py
```

What the scripts already do, and why (do not undo):

- fit at 48 kHz (FFT resample of the target); `frames` = real target length.
- loss per family: **noise voices train on band-pooled log power** (per-bin log
  magnitude is swamped by Rayleigh variance and smears flams); **pitched voices train
  on per-bin log magnitude (windows 256…4096) + 0.3 × a harmonic-track loss** —
  heterodyned amplitude of harmonics 1..10 along the *target's* phase track, 2 ms
  steps, -60 dBFS floor. The gate stays `compare.py`.
- baseline = spec midpoints; restarts = measured seed + midpoint + random draws (random
  draws keep the measured pitch curve — a random sweep never lands in the basin); top
  `--keep` by loss get coordinate descent with contraction; the winner gets a final tight
  pass. `--start <json>` seeds from a previous winner. In practice the measured seed won
  every round; restarts are insurance, not the search.
- `--only a,b,c` refines named scalars with the rest frozen. Use it for blocks the gate
  cannot see (below).
- writes `target.wav`, `initial.wav`, `learned.wav`, `ab.wav`, `recovered_params.json`
  with `pinned`, the gate number, and (pitched) the synth-minus-target harmonic table.

Reading a round — `deficit_table.py` (any voice) and `clap_residual.py` (noise):

- **harmonic table** (pitched): per harmonic and time, synth minus target in dB. This
  is the diagnostic the gate lacks; a round is not done while a harmonic that sits above
  the floor is off by 5 dB. Misses at f0 > 300 Hz are usually pitch-model error moving a
  harmonic out of its own window, not level.
- **deficit table**: target level and (target − learned) per band and time window.
  A uniformly short band = missing capacity (add a zero-default block); a band above the
  source's Nyquist = capture band-limit (fixed output lowpass); wrong timing = loss
  problem. A high band short by 10–30 dB from 30 ms on is recording hiss.
- **onset samples**: the synth must open the way the sample does. A large first-sample
  value the target lacks is a fitted click gone wrong — cap it, it costs nothing.
- **metric noise floor** (noise voices): the gate between the same patch with two
  noise seeds is ~0.46; report excess over floor; done ≈ excess < 0.03. On a
  full-band noise wash (the 909 hat) the floor is ~2.4 against a 2.9 baseline
  and every round scores under it: the gate is blind, read the pooled loss and
  the deficit table instead (`fit_hat.py` prints `floor` and `excess`).
- **metal modes**: measure the narrow tail peaks and fit them as deterministic
  decaying sines with frequency bounded ±5 % of the measurement, never as
  noise-excited high-Q resonators (the fitter chases the noise realisation and
  walks the frequencies to their bounds). Add the per-bin long-window term
  (`--fine-weight`) or the pooled loss trades a mode for broad fill.
- **pinned params are a diagnosis**: a bank decay pinned at its floor means that
  harmonic is being manufactured elsewhere (the tanh); widen only if the measurement
  supports it. Unused blocks (gain → 0) stay in as inert capacity.
- **blocks the gate cannot see** (hiss at -70 dBFS): the per-bin metric both ignores
  them and *penalises* them (any noise realisation differs from the target's), so the
  full fit always drives them to zero and a pooled term let into the full fit drags the
  body into a worse basin. Fit them alone: freeze the converged body, `--only
  hissCutoff,hissAmp,hissDecay --harmonic-weight 0 --pooled-weight 20` against the
  high-band (≥ 2.5 kHz, ε = 1e-6) pooled term, check the >4 kHz RMS envelope in the
  deficit table, and expect the gate to *rise* a little. Say so in the handoff.

Iterate: run → diagnose → fix root cause → run, seeding from the previous winner.
Converged when two rounds agree to ~0.001 on the loss with nothing meaningful pinned
and the harmonic table is flat. Then the ear: write an A/B that includes the previous
version (`target / old / new`), tell the user where it is, and wait for the verdict
before closing the bead. Character feedback ("it sounds recorded", "there's a rasp")
maps to the deficit table, not to the gate.

## 4. Port to eseq

Instrument = `content/instruments/Drums/<Name>/` (`dsp.lisp` + `ui.lisp`, presets at
`content/instruments/Drums/<Name>.presets`, auto-discovered). Templates next to this
file: `harmonic-kick-dsp-template.lisp` (pitched bank) and
`noise-voice-dsp-template.lisp` (noise), `hat-voice-dsp-template.lisp` (noise wash + sine mode bank). Identified scalars are `(param … @default
__KEY__ …)` with KEY = recovered_params key upper-cased; departure knobs (tune, sweep,
attack, decay, harm, bright, noise, hiss, drive, level) are no-ops at default so the
instrument boots AS the sample. Mirror the NumPy `render()` line by line. A departure
knob on a block identified at zero is dead weight — drop it, keep the scalars editable.

dgenlisp facts the port must absorb (verified 2026-09-03):

- `biquad` coefficients hardcode 2π/44100: pass `(* hz (/ 44100.0 samplerate))`;
  gain arg must be 1 (0 is silence); modes 0 LP, 1 HP, 2 = RBJ constant-peak BP.
- time ramp: `make-history` + `gswitch` on the trigger (see the hat template) gives
  t = 0 on the trigger sample then n/sr; `(accum …)` outputs 0 on the trigger sample and
  the next. Keep the **integer sample count** in the history and multiply by 1/sr
  once: summing 1/sr sample by sample drifts in float32 (0.35 cycles on a 5 kHz
  sine by 200 ms, 8.7e-3 max-abs on the hat; 2e-5 with the counter).
- `(noise)` scaled `(- (* (noise) 2) 1)` equals the fit's stream exactly; one `def`,
  reused, is one stream (the noise burst and the hiss share it, as in the fit).
- `floor`, `pow`, `pi` work; wrap phase before `sin`. `selector` is 1-based; string
  literals cannot contain `\"`.
- the fit's pitch is a host-pitch ratio: render the port check at the same `--pitch`
  the template divides by (261.63).

Prove parity (fills the template, writes dsp.lisp, renders through the audition harness):

```bash
export DGEN_RUNTIME_INCLUDE=$PWD/crates/sequencer/tools/dgen-toolchain/include \
       DGEN_BINARY_AUDIT_TOOL=$PWD/crates/sequencer/tools/DGenLisp-macos-arm64.dist/dgenlisp-macos-arm64/scripts/audit-dgen-dylib.sh
python3 tools/audition/synthid_port_check.py --run ~/code/swift/dgen/output/<sound>_vN \
   --template .claude/skills/identify-drum/<template>.lisp \
   --instrument "content/instruments/Drums/<Name>" \
   --fit-module ~/code/swift/dgen/Examples/SynthID/scripts/fit_<sound>.py
```

Expect identical gate numbers. Max abs < 1e-3 for short noise voices; a long tonal
tail shows float32 clock drift that grows smoothly with time (1.3e-3 at -47 dB relative
on a 460 ms kick) — fine. A one-sample envelope offset shows up as a large max-abs at
onsets with the gate unchanged; a wrong block shows up as a different gate. Run the
check once on the measured seed before the first fit round: it proves the template
compiles while the fit runs. Copy the template into the skill folder if the topology is
new.

UI: copy `Drums/Virus B BassDrum 23/ui.lisp` (`idvb23-*`) or `Drums/808 Clap/ui.lisp`:
PLAY/SHAPE departure knob panels in the left column, identified scalars as dense number
grids on the right (a 9-harmonic bank fits as two 5-wide grids at width 3.62). Every
parameter appears exactly once. Presets: first preset = the identified sound with
`params: {}`, then departures only; include a "Dry" preset (hiss/noise 0) so the user
can A/B the texture in the app. Layout test: copy
`metal_seq_fx_lisp_lays_out_virus_b_bassdrum23_controls` in
`crates/sequencer/src/ui/state_values/tests.rs`; regenerate its param list from the
shipping dsp.lisp with a regex over `(param name @default d @min lo @max hi` rather than
typing it:

```bash
cargo nextest run -p sequencer -E 'test(~lays_out_<name>)'
```

## 5. Close out

- `Examples/SynthID/HANDOFF_<SOUND>.md`: measurements, voice, run table (loss parts,
  gate, what changed and why), harmonic/deficit diagnostics before and after, final
  params, parity numbers, and which blocks were fitted alone.
- `bd comment` the bead naming both repos' files; both trees stay uncommitted unless
  asked. Do not close the bead on the numbers — the ear is the last gate.
- Tell the user where `ab.wav` (and the `target / old / new` A/B) is.
