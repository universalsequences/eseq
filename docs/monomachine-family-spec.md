# Monomachine Family v2 — spec / plan

Goal: a family of 4–5 separate instruments that capture the *spirit* of the Elektron
Monomachine (SOPHIE, Autechre: melty, gulpy, glassy, p-lock-driven) rather than a 1:1
emulation, modernized with our DSP power. Replaces/supersedes the earlier attempts in
`instruments/monomachine/` (digipro, dpro-wave-v2, dpro-dens-v1, fmplus*, superwave).

## Diagnosis: why the existing attempts lack depth

The core misunderstanding in every prior attempt: **the Monomachine's machines are
simple; the depth is in the shared per-track chain that comes after them.** Every MnM
track runs its machine through:

1. **AHD amp envelope** — attack / *hold* (fixed-time sustain) / decay + release.
   Percussive and gated by nature, not pad-shaped ADSR.
2. **Dual base/width filter** — one cutoff ("base") + a "width" that separates an HP
   and LP pair, each with its own resonance, **with a dedicated A/D envelope that can
   sweep both base and width**. A narrow band swept by envelope + LFO is *the* gulp.
   Width=0 collapses to a resonant 24 dB LP.
3. **Per-track FX page** — AM (amplitude modulation), SRR (sample-rate reduction,
   post-filter, aggressively lo-fi), 1-band EQ with ±36 dB(!) gain, tempo-locked
   track delay (1/256-note resolution) whose feedback loop interacts with the track.
4. **3 LFOs** — 11 shapes, free/trig/hold/one-shot modes, "interlace" (alternate
   waveform sample with zero), destinations include *other LFOs' speeds*.
5. **Portamento** on every track (mono voice → legato glide is a house sound).

Our four attempts each implement only a machine (oscillator page) and then bolt on the
identical generic skeleton: `tanh(drive)` → single biquad LP → ADSR. See
`monomachine-digipro/dsp.lisp:110-114`, `dpro-wave-v2/dsp.lisp:71-74`,
`dpro-dens-v1/dsp.lisp:98-101`, `fmplus-par-v1/dsp.lisp:134-137`. Specific gaps:

- **No base/width filter, no envelope on width** → no gulp, no vowel-ish motion.
- **Sustained ADSR defaults** → generic soft-synth envelope character.
- **No SRR / AM / big EQ / track delay** → none of the 12-bit glassy-nasty finish.
- **No VO machine at all** — the SOPHIE signature (formant/phoneme synthesis) is
  entirely missing from our set.
- **No SID machine** — the gritty ring-mod/sync/R+S character is missing.
- The digipro attempt fakes "wavetables" by crossfading five analytic waveforms;
  wave-v2/dens-v1 fixed this with real 512×N banks (keep that).
- Motion is hardwired (superwave's internal LFOs) instead of exposing the *right*
  `@mod` destinations (vowel, width, SRR, ratio) to the DAW mod system + p-locks.
- Mono output; polite parameter ranges everywhere (res ≤ 2.5, detune ≤ 40 cents).

FM+ note: our fmplus-par models PARALLEL. The expressive MnM FM machine is
**DYNAMIC** — modulator *frequency envelopes* (pitch-sweeping modulators) and a
feedback envelope. Time-varying ratios are what melt.

## Shared "MnM track chain" (the family identity)

A canonical macro block copied verbatim into each instrument's `dsp.lisp` (no include
support in the instrument lisp; keep the master copy in
`instruments/monomachine/CHAIN.md` and treat drift as a bug). Contents:

- `mnm_amp_env` — AHD+R: `attack_ms`, `hold_ms`, `decay_ms`, `release_ms`; hold as a
  fixed-time full-level stage after attack (build from adsr + a latch/timer, or
  two-stage envelopes). Defaults percussive.
- `mnm_filter` — HP biquad → LP biquad. Params: `flt_base` (Hz, @mod), `flt_width`
  (octaves above base for the LP corner, @mod; width big = wide open), `flt_res_lo`,
  `flt_res_hi`, plus a dedicated AD envelope with `env_to_base` and `env_to_width`
  depths (both bipolar, @mod). This is the gulp engine — every instrument gets it.
- `mnm_am` — amplitude modulation by an internal sine, `am_rate` (Hz, @mod, up to
  audio rate ~2 kHz for sidebands) + `am_depth` (@mod), optional keytrack of rate.
- `mnm_srr` — sample-rate reduction post-filter: `latch` at a variable clock,
  `srr` param 0..1 mapping to hold-rate from clean → ~500 Hz. @mod. Optional 12-bit
  quantize baked in per machine.
- `mnm_eq` — one peak biquad, `eq_freq` + `eq_gain` (±24 dB or more, @mod). Abused
  as a formant/honk control, not a mixing EQ.
- `glide_ms` — one-pole slew on the pitch input (mono legato glide). Verify pitch
  input behaves stepwise-legato per voice; if the host reuses the voice for legato
  notes this works as-is.
- Stereo finish — cheap width per machine (e.g. haas/phase-offset dual render or a
  short modulated delay on one side), `width` param. Two `out` channels.

Character rules for the family: percussive envelope defaults; ranges allowed to get
nasty (res into self-oscillation territory, SRR to destruction, EQ huge); parameters
that are *indices* (vowel, wave, ratio) stay stepped so p-locks jump distinctly.

## The instruments

### 1. `monomachine/vox` — VO-6 spirit (highest priority; the SOPHIE machine)
Formant vocal synthesis, no samples:
- **Glottal source**: pulse train w/ shape (LF-model-ish brightness param) morphable
  to noise (`breath`, @mod). `growl` = sub-audio AM on the glottis (@mod).
- **Formant bank**: 3–5 parallel bandpass biquads. A vowel table (a e i o u + æ ø
  etc., ~10 entries) of formant freqs/gains/Qs; `vowel` (stepped index, @mod) +
  `vowel_morph` (continuous interpolation to next, @mod) — LFO on morph = the classic
  wigging-out vowel babble. `formant_shift` (@mod) scales all formants independent of
  pitch (the SOPHIE helium/glassy trick — modernization the VO-6 didn't have).
- **Consonant generator**: on-trigger noise burst through a shaped filter; a stepped
  `consonant` selector (~12–20 archetypes: s/sh/t/k/p/f/r...), `cons_len_ms`,
  `cons_level`, `sibilance` (HF tilt). Retrigger-safe.
- Chain: full MnM chain (filter width + formants stack beautifully).

### 2. `monomachine/grit` — SID spirit
- Osc pair: tri/saw/pulse/mixed/noise (stepped `waveform` selector, @mod) with
  **hard sync, ring mod, and R+S** (`mode` selector) against a second osc with
  stepped-ratio tune (semitone steps, @mod → p-lockable arp intervals).
- Aggressive PWM (`pw`, @mod), 8-bit-ish quantize pre-filter, and a nonlinear
  resonant filter stage (drive inside the resonance path) for the SID scream.
- `interlace` gimmick: alternate osc output with zero at a settable rate (SID/MnM
  LFO trick) — cheap, weird, very Autechre.
- Chain: full MnM chain; SRR defaults hotter here.

### 3. `monomachine/melt` — FM+ DYNAMIC spirit
FM where *ratios move*:
- Carrier + 2 modulators (sine or wavetable-as-operator, reuse the 512×64 user bank).
- Per-modulator **frequency envelope** (AD, bipolar depth in octaves, @mod) — the
  modulator sweeps through inharmonic territory every note = melt/gulp.
- Feedback with its own AD envelope (@mod depth); continuous (non-listed) ratio
  params, stepped `ratio_snap` toggle for clean-FM presets.
- Keep the listed-ratio selector from fmplus as an optional quantize.
- Chain: full MnM chain (filter-width envelope on FM = liquid).

### 4. `monomachine/wave3` — DPRO spirit, modernized (upgrade of dpro-wave-v2)
- Keep the real 512×32 factory bank + scan slew from wave-v2.
- Add what real DPRO WAVE had that v2 lacks: **phase morph** (the MnM "wave morph"
  distorts readout phase), 12-bit quantize of table output, hard-sync already there.
- Add `wave_stack`: 2nd table voice at stepped interval (borrow dens-v1's pch logic)
  so WAVE and DENS collapse into one deeper instrument.
- Chain: full MnM chain replacing the current generic tail.

### 5. (Optional) `monomachine/ens` — SWAVE ENS spirit
Only if dens-v1 + wave3 leave a hole: 8-osc ensemble w/ internal 4-note chord +
BBD-style chorus (we have dimension-chorus DNA). Defer; superwave/dens cover much
of it once they get the chain.

## Execution order

1. **Chain first**: build + verify the macro block standalone via the C harness
   (compile a test dsp.lisp, measure gulp sweep, SRR spectra) before any machine.
2. `vox` (biggest gap, biggest payoff) → 3. `grit` → 4. `melt` → 5. `wave3` retrofit.
3. Each ships with `ui.lisp` + 8–16 presets aimed at reference sounds (SOPHIE rubber
   /balloon/vocal pops, Autechre metallic melt, MnM lush ens pads, SID arps).
   Preset bank format + audition harness per custom-instrument-workflow.
4. Old instruments stay untouched until the new family is auditioned; then decide
   deprecation.

## Status (2026-07-02)

Built and harness-verified: shared chain (all 12 behavior checks), `vox`, `grit`,
`melt`, `wave3` — each with dsp.lisp + ui.lisp (layout tests
`metal_seq_fx_lisp_lays_out_mnm_*` in state_values.rs) and a preset bank.
Chain master copy lives in `instruments/monomachine/CHAIN.md`.

Found along the way: **dgen `floor` is a no-op on runtime values** in the current
build (constant-folds literals only). Workaround baked into the chain as
`mnm_floor` (`(- x (% x 1))`, correct for negatives). Older instruments that use
bare `floor` on params (digipro, dpro-wave-v2, fmplus) are silently un-quantized.
Not yet auditioned in the DAW; old monomachine instruments left untouched.

## Open questions
- Legato/glide: confirm host voice reuse makes in-instrument pitch slew behave as
  portamento (else needs host support).
- Track delay: leave to the FX rack (space-echo exists) or add a mini tempo-synced
  in-instrument delay with @mod time for per-note p-locked smears? Leaning: leave
  out of v1, revisit after auditioning.
- Consider adding an `(include ...)` form to the instrument lisp loader if chain
  drift becomes painful.
