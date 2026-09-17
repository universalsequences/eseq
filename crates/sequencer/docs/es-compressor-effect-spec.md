# ES Compressor — builtin effect spec

**Status:** built 2026-09-16. DSP in `src/effects/es_compressor_dsp.lisp`,
host glue in `src/effects/es_compressor.rs`, panel in
`content/ui/effects/builtin/es-compressor.lisp`.

## What it is

A sampler-style "sustain" compressor: the kind of heavy, low-threshold,
auto-makeup compression that makes decaying material feel sustained and
glued while drum transients still punch, followed by a soft-knee saturator
and a dark minimum-phase tone filter. It is a builtin whose DSP body is
bundled dgenlisp, hosted through the same compile/cache path as custom
effects (the Filter Table pattern), but with no per-instance assets: every
curve is a closed-form formula and all tuning constants live in the TUNING
block at the top of the lisp source.

## Signal flow

1. **Detector.** Stereo link on the louder channel, 0.25 ms dyadic sliding
   maximum (lookahead), converted to dB, plus the detector trim.
2. **Static curve.** Threshold sweeps -36 to -66 dB with `amount`
   ("sustain"). Quadratic soft knee (6 dB), then a *saturating* reduction
   curve `max * (1 - exp(-slope * over / max))`. The ceiling `max` grows from
   8 dB to 256 dB across the knob: a low ceiling lifts quiet tails hard while
   loud transients earn almost no extra reduction (punch), a high ceiling is
   near linear (maximum sustain).
3. **Envelope.** The error between the target and the mean of the previous
   1 ms of reduction sets the time constant (exponential speedup: 4 dB per
   e-fold on attack, 8 dB on release) and an aim that overshoots the target
   (1.5x to 7x the error with sustain squared, capped at 18 dB), so the gain
   grabs and settles instead of creeping. The attack/release knobs are the
   effective small-error time constants (defaults 40 ms / 120 ms).
4. **Level-gated grip.** Inside a hot window from -20 to -8 dBFS the time
   constants shrink by up to `exp(-5)`, letting the gain ride inside the
   waveform of loud bass and kicks (odd harmonics from the compressor
   itself). Quieter material keeps the smooth envelope, so the effect reads
   as compression rather than distortion.
5. **Gain.** Auto makeup gives unity for a steady -23 dBFS input. `mix`
   scales (makeup - reduction) in dB, blends the shaper, and scales the tone
   curve in dB. It is not a conventional dry/wet.
6. **Shaper.** Unity below 0.75, then the rational soft clip
   `s / (1 + s^n)^(1/n)` (hardness n = 4) up to a ceiling of 1.02. `drive`
   adds gain in front and removes 70% afterwards.
7. **Tone.** +1.5 dB shelf below 200 Hz and a 12th-order 11.5 kHz lowpass,
   evaluated per hop on a 4096-bin grid at the host rate, turned into a
   minimum-phase impulse via the real cepstrum, tapered, and applied with
   8192-point overlap-save at a 256-sample hop. Old/new kernels crossfade
   across the hop when `mix` moves.

## Parameters

| Name | Range | Default | Notes |
|---|---|---|---|
| `amount` | 0–100 | 50 | Sustain macro: threshold, slope, curve ceiling, makeup. 0 is not bypass. |
| `attack` | 1–200 ms | 40 | Exponential knob taper. |
| `release` | 20–2000 ms | 120 | Exponential knob taper. |
| `mix` | 0–1 | 1 | dB-domain scaling of gain, shaper and tone. |
| `drive` | 0–24 dB | 0 | Into the shaper, 70% compensated. |
| `input-db` | -24–24 dB | 0 | |
| `output-db` | -48–6 dB | -6 | The tone FIR can push peaks ~2 dB over the shaper ceiling; the default absorbs it. |
| `detector-db` | -36–36 dB | 0 | Detector-only offset. Negative backs the compressor off and lets makeup drive the shaper. |

## Latency

The source declares `(effect-latency (+ 255 (max 1 (min 127 (round (* 0.00025 samplerate))))))`:
255 samples of overlap-save block delay plus the lookahead window (267 at
48 kHz, 279 at 96 kHz). The host's delay compensation reads it from the
compiled manifest; no Rust registration is involved. This is the first
factory effect to use the declaration.

## Host integration

- `dgen_builtin::NAMES` / `find` register the name, bundled source and
  `DGenSourceOrigin::BuiltinEsCompressor`.
- `initialize_dgen_builtin_node` has a no-op arm (nothing to seed).
- `attack` / `release` get `ParamScaling::Exponential` in
  `EffectDescriptor::from_lisp_manifest_with_asset_base`.
- Everything else (browser listing, `builtin:` project names, undo
  retention, PDC) is generic.

## Verification

- `effects::es_compressor::tests` pin the param set, the asset-free
  invariant, and the declared latency at five sample rates.
- `state_values::tests::metal_seq_fx_es_compressor_layout_contains_knobs`
  covers the panel; `ui/capture-fixtures/es-compressor-panel.lisp` renders
  it through the real add path.
- Behavioural tuning was done offline against a private reference on
  chord-plus-drums material and 110 Hz sines at several levels (envelope
  within ~1 dB, third harmonic -17 dB at -6 dBFS and below -40 dB at
  -12 dBFS and quieter at amount 100).

## Cost

Roughly 7% of one core per instance at 48 kHz: the tone kernel is rebuilt
every hop even when `mix` is static, and each channel runs two 8192-point
inverse FFTs per hop for the crossfade. Rebuilding only on `mix` change is
the obvious optimization if instances multiply.
