# Filter Table Custom Effect Spec

Status: draft  
Target effect folder: `crates/sequencer/effects/filter-table/`

## Goal

Build a DGenLisp stereo audio effect inspired by Kilohearts Filter Table's
publicly documented behavior: a scanned wavetable frame becomes a morphing
filter response applied to incoming audio.

This is not a clone of Kilohearts' private DSP or factory content. The
implementation must use original tables, user-owned imported tables, or
procedurally generated tables shipped in this repo.

## Research Basis

Kilohearts documents the following user-facing behavior:

- A wavetable frame is transformed into a filter shape and applied to incoming
  audio.
- Scanning through the table smoothly changes the filter shape.
- The product includes a 3D table visualization, a spectrum plot for the
  selected frame, and a table selector/editor.
- `Cutoff` defines the frequency of harmonic 24 in the wavetable.
- `Resonance` emphasizes or attenuates peaks and troughs in the filter shape.
- `Mix` is dry/wet; phase mode can affect dry/wet phasing.
- Phase modes are `MINIMUM`, `LINEAR`, `ORIGINAL`, and `RAW`.

Sources:

- https://kilohearts.com/docs/filter_table
- https://kilohearts.com/products/filter_table
- https://kilohearts.com/blog/introducing_kilohearts_filter_table
- https://kilohearts.com/docs/wavetables

Kilohearts does not publish the exact transfer-function construction,
resonance law, table normalization, or phase reconstruction. This spec defines
a local implementation that targets the same interaction model while staying
honest about the non-public parts.

## Local Constraints and Existing Support

Custom audio effects in this repo are folder artifacts:

```text
crates/sequencer/effects/<name>/
  dsp.lisp
  ui.lisp
  *.json
```

The DGenLisp effect source must be plain top-level forms, not wrapped in
`defeffect`. A stereo effect declares left/right inputs and outputs. If any
effect parameter is `@mod true`, the four host modulation inputs must be
declared before any sidechain inputs.

Existing spectral effects establish the runtime pattern to reuse:

- Use host-compatible hop size `512`.
- Use `N=2048` for the first production version unless profiling proves that
  `1024` is necessary.
- Use `sqrt(hann(2048)) * 0.70710678` for analysis/synthesis windows at
  75 percent overlap, matching the existing unity-COLA convention in
  `spectral-notch-phaser`, `spectral-vox`, and `spectral-bloom`.
- Mix wet against an STFT bypass, not raw input, so dry/wet remains aligned in
  linear/original/raw STFT modes.
- Hop-hold every parameter-derived scalar before it touches tensor math. This
  prevents tensor/IFFT chains from demoting to per-sample execution.

DGen/DGenLisp already has the required primitives:

- `buffer`, `fft`, `ifft`, `overlap-add`, `hann`
- destructuring `def`
- `polar-fft`, `rect-fft`, `complex-mul`
- `tensor @file`, `sample`, `gather`, `cumsum`, `conv1d`
- `hop-hold`, tensor `noise`, tensor history
- `audio-tensor` for offline/import tooling, though V1 should convert imports
  to JSON table banks before DSP compile

There is also an upstream DGen low-level test named
`FilterTableTests.testFilterTableCoreUsesHopHeldTableFrame`, which validates
the core idea: sample a table row at hop rate, FFT it, complex-multiply the
input spectrum, IFFT, and overlap-add.

## Product Semantics to Preserve

The effect should expose these controls in V1:

| Param | Range | Default | Notes |
|---|---:|---:|---|
| `frame` | `0..1` | `0` | Scans table frames. Host-modulatable. |
| `cutoff` | `40..18000 Hz` | `1000` | Frequency assigned to harmonic 24. Host-modulatable. |
| `resonance` | `-1..1` | `0` | Negative compresses spectral contrast; positive expands it. |
| `phase_mode` | `1..3` | `1` | V1 exposes `1=linear`, `2=original`, `3=raw`. `0=min` is reserved for V2. |
| `mix` | `0..1` | `0.7` | Dry/wet against STFT bypass for STFT modes. |
| `output` | `0.25..2` | `1` | Final gain. |

Optional later controls:

- `table` selector for several bundled banks.
- `smooth` for extra spectral smoothing.
- `keytrack` only after note/modulator routing can supply pitch reliably to an
  audio effect slot.

## Asset Format

Use a dedicated filter-table asset layout, not the oscillator wavetable layout.

```json
{
  "shape": [64, 2048],
  "data": [
    [ ... 2048 samples for frame 0 ... ],
    [ ... 2048 samples for frame 1 ... ]
  ]
}
```

Interpretation:

- Axis 0 is frame index.
- Axis 1 is one full-cycle frame/kernel at `N=2048`.
- DGenLisp reads it with `(peek-row table frame-index)`, which interpolates rows
  and returns a `[2048]` signalTensor. (Do not use `sample` here — `sample` is
  the gen-style *scalar* read at a normalized 0..1 phase.)

Do not use the oscillator-table sample-major `[samples, waves]` convention for
this effect. That convention is correct for `(peek table idx wave)` /
`(sample table phase wave)`, but it is wrong for table-row scanning with
`(peek-row table frame)`.

Required helper tensors:

```text
tables/default.json        # shape [frames, 2048]
fold_norm_2048.json        # folded 0..1 Nyquist position, symmetric
bin_sign_2048.json         # 0, +1 positive bins, 0 at Nyquist, -1 mirror
```

Prefer `fold_norm` plus `samplerate` over baked-Hz assets so the effect remains
sample-rate portable:

```lisp
(def bin-hz (* fold-norm (* 0.5 samplerate)))
```

Add a small generator script, for example:

```text
crates/sequencer/effects/filter-table/generate_tables.py
```

The script should:

- Generate folded helper tensors for the selected `N`.
- Generate a few original procedural banks, e.g. formants, combs, vowel-like
  masks, spectral notches, and noisy texture sweeps.
- Optionally import user-supplied single-cycle wavetable WAV/JSON files into
  `[frames, N]`, with DC removal, resampling, RMS normalization, and optional
  phase alignment.
- Never vendor Kilohearts factory content.

## DSP Design

### Analysis

Use the established stereo STFT skeleton:

```lisp
(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(def mod1 (in 3 @name mod1 @modulator 1))
(def mod2 (in 4 @name mod2 @modulator 2))
(def mod3 (in 5 @name mod3 @modulator 3))
(def mod4 (in 6 @name mod4 @modulator 4))

(param frame     @min 0    @max 1     @default 0.0 @mod true @mod-mode additive)
(param cutoff    @min 40   @max 18000 @default 1000 @unit Hz @mod true @mod-mode additive)
(param resonance @min -1   @max 1     @default 0.0 @mod true @mod-mode additive)
(param phase_mode @min 1   @max 3     @default 1)
(param mix       @min 0    @max 1     @default 0.7)
(param output    @min 0.25 @max 2     @default 1.0)

(def win (* 0.70710678 (sqrt (hann 2048))))
(def frame-l (* (reshape (buffer in-l 2048 512) @shape [2048]) win))
(def frame-r (* (reshape (buffer in-r 2048 512) @shape [2048]) win))
(def (xre-l xim-l) (fft frame-l @N 2048 @backend accelerated))
(def (xre-r xim-r) (fft frame-r @N 2048 @backend accelerated))
```

### Table Frame Selection

```lisp
(def table (tensor @shape [64 2048] @file "tables/default.json"))
(def frame-count 64)
(def frame-h (hop-hold (* (clip (mod frame) 0 1) (- frame-count 1)) 512))
(def table-frame-raw (peek-row table frame-h))
```

Normalize the selected frame before turning it into a response:

```lisp
(def table-frame-centered (- table-frame-raw (mean table-frame-raw)))
(def table-rms (sqrt (+ (mean (* table-frame-centered table-frame-centered)) 0.000001)))
(def table-frame (* table-frame-centered (/ 1 table-rms)))
(def (tre tim) (fft table-frame @N 2048 @backend accelerated))
(def (tmag tphase) (polar-fft tre tim))
```

### Cutoff Mapping

Kilohearts defines cutoff as the frequency of harmonic 24. Therefore, for an
audio FFT bin at frequency `f`, the corresponding table harmonic index is:

```text
table_harmonic = 24 * f / cutoff
```

Use the folded positive-frequency axis, clamp to the usable positive spectrum,
and manually lerp because `gather` truncates fractional indices:

```lisp
(def fold-norm (tensor @shape [2048] @file "fold_norm_2048.json"))
(def bin-sign  (tensor @shape [2048] @file "bin_sign_2048.json"))

(def cutoff-h (hop-hold (max 40 (mod cutoff)) 512))
(def bin-hz (* fold-norm (* 0.5 samplerate)))
(def harmonic-pos (min 1023 (max 0 (* 24 (/ bin-hz cutoff-h)))))
(def h0 (floor harmonic-pos))
(def hf (- harmonic-pos h0))

(def tmag-warp (+ (* (gather tmag h0) (- 1 hf))
                  (* (gather tmag (+ h0 1)) hf)))
(def tphase-pos (+ (* (gather tphase h0) (- 1 hf))
                   (* (gather tphase (+ h0 1)) hf)))
(def tphase-warp (* bin-sign tphase-pos))
```

### Resonance and Gain Law

Convert table magnitude to a bounded, unity-centered log-gain curve:

```lisp
(def logmag (log (+ tmag-warp 0.000001)))
(def centered-logmag (- logmag (mean logmag)))
(def res-scale (hop-hold (exp (* 2.0 (mod resonance))) 512))
(def gain-log (clip (* centered-logmag res-scale) -3.0 3.0))
(def gain (exp gain-log))
```

This is a local design choice. It preserves average loudness and makes
resonance a spectral-contrast control:

- `resonance < 0`: flatten the table response.
- `resonance = 0`: use the normalized table contrast.
- `resonance > 0`: exaggerate peaks/troughs.

If testing shows the effect is too aggressive, change the exponent coefficient
or clip range, not the cutoff mapping.

### Phase Modes

Implement mode selection with tensor masks, not branching:

```lisp
(def mode-h (hop-hold phase_mode 512))
(def is-min  (lte mode-h 0.5))
(def is-lin  (* (gt mode-h 0.5) (lte mode-h 1.5)))
(def is-orig (* (gt mode-h 1.5) (lte mode-h 2.5)))
(def is-raw  (gt mode-h 2.5))
```

V1 phase behavior:

- `linear`: magnitude-only filter, zero phase in the STFT domain.
- `original`: use the warped table phase with `bin_sign` to preserve
  conjugate symmetry.
- `raw`: use the selected frame's complex FFT response with only normalization
  and cutoff warping. It is intentionally more artifact-prone.
- `minimum`: not exposed in V1. If a stale preset or hand edit sets
  `phase_mode < 0.5`, the DSP may defensively fall back to linear behavior, but
  the UI must not label that as minimum phase.

Required V2 for true `minimum`:

- Build a minimum-phase response from `gain` using real cepstrum:
  1. `log_gain = log(gain)`.
  2. IFFT `log_gain` to real cepstrum.
  3. Double positive quefrency bins, zero negative quefrency bins, preserve DC.
  4. FFT back to obtain complex log response.
  5. Use the imaginary component as phase.
- This still has STFT window latency. Exact Kilohearts-style "no latency" would
  require a causal time-domain dynamic FIR path:
  `buffer(input, K)` dot `min_phase_kernel`. That is expressible in DGen with
  signalTensor arithmetic, but it is much heavier and needs profiling before it
  becomes a production mode.

V1 filter construction:

```lisp
(def phase-original tphase-warp)

; Raw mode starts from the warped table phase but skips log centering in favor
; of direct average-normalized table magnitude. Keep bounded output gain.
(def raw-gain (clip (* tmag-warp (/ 1 (+ (mean tmag-warp) 0.000001))) 0 8))

(def phase-selected (+ (* is-orig phase-original)
                       (* is-raw phase-original)))
(def gain-selected (+ (* (+ is-min is-lin is-orig) gain)
                      (* is-raw raw-gain)))

(def hre (* gain-selected (cos phase-selected)))
(def him (* gain-selected (sin phase-selected)))
```

### Spectral Application and Output

```lisp
(def (yre-l yim-l) (complex-mul xre-l xim-l hre him))
(def (yre-r yim-r) (complex-mul xre-r xim-r hre him))

(def bypass-l (overlap-add (* (ifft xre-l xim-l @N 2048 @backend accelerated) win) 512))
(def bypass-r (overlap-add (* (ifft xre-r xim-r @N 2048 @backend accelerated) win) 512))
(def wet-l (overlap-add (* (ifft yre-l yim-l @N 2048 @backend accelerated) win) 512))
(def wet-r (overlap-add (* (ifft yre-r yim-r @N 2048 @backend accelerated) win) 512))

(out (* output (mix bypass-l wet-l mix)) 1 @name left)
(out (* output (mix bypass-r wet-r mix)) 2 @name right)
```

Add `tanh` only if tests show the response can clip under ordinary input. A
hard safety clip would mask gain-law bugs, so prefer bounded `gain-log` first.

## UI Design

Ship a hand-written `ui.lisp` using current effect UI blocks.

Suggested layout:

- Block `FILTER TABLE`: `frame`, `cutoff`, `resonance`, `phase_mode`
- Block `OUT`: `mix`, `output`
- Optional readout block later for table name and visual spectrum

V1 can use numeric/knob controls for `phase_mode`. A later UI pass should use a
dropdown or segmented control with labels `lin`, `orig`, `raw`; add `min` only
after the minimum-phase path is implemented.

The Kilohearts-style 3D graph/spectrum plot is not required for V1 DSP
correctness. If added, prefer extending or reusing the existing
`wavetable-viewer`/spectrogram widget patterns rather than encoding a bespoke
viewer in this effect.

## Implementation Plan

1. Add `crates/sequencer/effects/filter-table/generate_tables.py`.
2. Generate helper tensors and one original default bank.
3. Add `dsp.lisp` using the V1 STFT design above.
4. Add `ui.lisp` with compact lego-style controls.
5. Add a Rust render test that compiles the saved effect source, initializes
   tensor assets from the effect directory, renders deterministic input, and
   asserts:
   - non-silent output,
   - no clipping under default params,
   - `mix=0` matches STFT bypass/dry within tolerance,
   - `mix=1` differs from bypass,
   - moving `frame` changes the rendered spectrum.
6. Add a DGen/DGenLisp regression test if missing in the bundled DGen repo:
   table row 0 is silent, row 1 is impulse/pass-through; modulated/hop-held
   frame switch changes output only on hop boundaries.
7. Run targeted validation:

```sh
cargo test -p sequencer lisp_host::tests::filter_table_custom_effect_renders -- --nocapture
swift test --package-path ~/code/swift/dgen --filter FilterTableTests
```

If DGenLisp or tensor loading changes are needed, rebuild the local DGenLisp
tool before host tests:

```sh
./rebuild_dgenlisp_tool.sh
```

## Acceptance Criteria

- The effect compiles from its saved folder with all JSON assets resolved
  relative to `dsp.lisp`.
- Default render is audible and below full-scale peak.
- `cutoff=1000` places table harmonic 24 at roughly 1 kHz in the applied
  response. A sine/noise FFT test should verify the response peak/trough moves
  proportionally as cutoff changes.
- `frame` scanning is smooth at hop boundaries and does not force per-sample
  FFT/IFFT execution.
- `linear` mode has the smoothest dry/wet blend because bypass and wet are
  STFT-aligned.
- `original` mode preserves obvious phase-derived coloration from asymmetric
  source frames without breaking conjugate symmetry.
- `raw` mode is allowed to sound rough, but it must not explode numerically.
- Any shipped procedural table bank is original and reproducible from the
  generator script.

## Open Risks

- Exact Kilohearts `minimum` behavior is not implementable honestly in V1 with
  the existing STFT effect route. A minimum-phase spectral response is possible,
  but documented "no latency" needs a separate causal dynamic-FIR mode.
- Kilohearts' resonance curve is private. The proposed log-contrast law is
  stable and musical, but it is not known to match the commercial product.
- Runtime table selection is limited unless multiple banks are compiled into
  the effect or the host gains an effect-asset picker/reload path.
- The UI graph/spectrum visualizer is deliberately deferred. It should not
  block the DSP effect, but the product will feel less complete until a viewer
  exists.
