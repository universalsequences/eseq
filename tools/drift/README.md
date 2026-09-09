# Drift filter and input-level measurements

Native reference: Ableton Live 12.4.5 Drift, high quality enabled, engine and
export at 48 kHz. Tracking: eseq-at95, eseq-96f1. The measurements now inform the
production Digi Drift filters and oscillator-level drive. The user explicitly
accepted a native-rate emulation without oversampling or a 1:1 native match.

The production instrument uses the measured Type-I SVF response and nonlinear
feedback, a four-pole Sallen-Key approximation for Type II, a downstream
resonant high-pass, and summing saturation before envelope/volume gain. The
Drive parameter is removed; oscillator gains supply the drive. See the final
implementation section in `measurements/findings.md` for limits and checks.

## Reference behavior

[Ableton's manual](https://www.ableton.com/en/manual/live-instrument-reference/#drift)
documents oscillator-level-driven saturation before and after filtering. There
is no separate Drive control in the inspected device or saved parameter schema.
Type I is DFM-1 (12 dB/octave, distortion inside feedback); Type II is Cytomic
MS2 (24 dB/octave, Sallen-Key with resonance soft clipping). These descriptions
identify behavior to investigate, not enough equations to implement the models.

## Capture

Save a Copy of the current Live set under `reference/` before doing anything
else. The template must contain exactly one top-level Drift track with a MIDI
clip. The generator never changes it and refuses to overwrite an existing output.
It removes all other tracks, return tracks, other devices, clip/track automation,
and source notes in the generated copy. Device and track output gains are
explicit, pan centered, filter/pitch/shape modulation and drift disabled,
oscillator retrigger enabled, one Poly voice per note, velocity-to-volume zero,
and amp sustain one. Every parameter and the input/set hashes enter the ledger.
The preserved template is an inspected user patch, **not** an initialized preset.

```sh
python3 tools/drift/make_reference.py /path/to/preserved-session.als \
  tools/drift/reference/levels.als --batch levels
python3 tools/drift/make_reference.py /path/to/preserved-session.als \
  tools/drift/reference/response.als --batch response
python3 tools/drift/make_reference.py /path/to/preserved-session.als \
  tools/drift/reference/summing.als --batch summing
```

Open each generated set in Live and export **All Individual Tracks**, start
**1.1.1**, length **10.0.0** (20 seconds at 120 BPM), **48 kHz / stereo / WAV /
32-bit float**, **No Dither**. Disable normalization, render-as-loop, return/main
effects, analysis files and MP3. Save with the same base name as the set, beside it:
`levels <track-name>.wav` / `response <track-name>.wav`. Main renders are unused.
The duration must be verified each time; Live remembers export settings separately.

- Levels: 76 tracks, including sine/saw bypasses; both filter types at 1 kHz,
  resonance 0 and 0.8, input levels -24/-12/-6/-3/0/+3/+6 dB; independent output
  volume controls and duplicate captures.
- Response: 38 tracks, including low-level sine/saw bypasses; both types at
  250/1000/4000 Hz, resonance 0/0.5/0.8/1; three high-pass settings; two sine
  oscillators a fifth apart at -6/0/+6 dB, filtered and bypassed.
- Summing: 20 tracks to investigate compression with the filter bypassed:
  two-oscillator input at three gain levels and three output volumes, solo
  oscillator 2 controls, both filtered sums at three output volumes, and bypass
  calibration sources. This batch was added after the first two exposed the
  high-level bypass compression.
- Linear: 55 tracks, including -48/-60 dB saw controls and both filters at
  250/1000/4000 Hz and resonance 0/0.5/0.8/0.95, plus three amp sustain controls.
  **These very quiet saw captures do not have enough signal above the noise
  floor to identify a full small-signal response.** See the diagnostic report.
- Ordering: 10 tracks, including paired -6/+6 dB two-oscillator bypasses with
  ascending, descending, repeated-low and repeated-high notes. These separate
  pitch effects from time/order effects. Generate with `--batch ordering`.
  Generate the quiet controls with `--batch linear`.
- Sine-response: 56 tracks, prepared with `--batch sine-response`, using sine
  notes below/near/above each cutoff at -24/-36 dB, with matching bypasses.
  Exported and analyzed after restarting Live on September 8.
- Highpass: 28 tracks (`--batch highpass`), including matched -24/-36 dB
  sine controls at MIDI 12/24/36/48/60, both types, HP 10/15/20/30/40/100 Hz,
  LP 19999 Hz, resonance zero. These distinguish a lower HP clamp from a
  type-dependent low-frequency response elsewhere in the path.
- Highpass-drive: 29 tracks (`--batch highpass-drive`), both types at LP 1 kHz,
  resonance zero, HP 20/1000/4000 Hz, input -24/-6/0/+6 dB and bypass controls.
  Exported and analyzed to determine HP/saturation ordering.
- Character: 67 tracks (`--batch character`), loud sine/saw inputs at 250/4000 Hz
  and resonance 0/0.5/0.8 for both types, plus finer Type-II resonance steps with
  matched quiet sine controls. Exported and analyzed September 8 (335 note windows).
- Resonance-law: 29 tracks (`--batch resonance-law`), Type II at 1 kHz with
  resonance steps of 0.05, matched -48 dB sine controls at pitches bracketing
  cutoff, and independent -60 dB high-resonance controls. This brings the corpus
  to 463 tracks and 2,315 held-note windows. One Live export filename acquired
  a `-1` suffix; its explicit original-name/hash mapping is preserved in
  `reference/resonance-law-export-names.json`.
- Default tracks play MIDI notes 36/48/60/72/84 at velocity 100, 2.5-second holds,
  with sufficient silence between them for release. Measurement windows are
  0.5–1.5 seconds after each onset. Sine-response, highpass, and ordering cases
  specify their own pitch sequences in the ledger.

`linear-clean` is an independent re-export of the exact `linear.als` set with
No Dither selected; its `.als` and ledger are byte-identical copies. Earlier
exports used the Triangular setting at 32 bits. Live's dialog says dithering
applies below 32 bits; the independent export confirms that disabling it does
not remove the observed quiet-signal noise. `ordering` uses No Dither.

`reference/` is ignored: preserve these sets and float WAVs locally. Compact
reports in `measurements/` include hashes and complete per-case native values.
WAVs may be losslessly gzip-compressed to save disk space; the analyzer accepts
`<name>.wav.gz` when `<name>.wav` is absent. Reported hashes and byte sizes always
describe the original uncompressed WAV. The first batch was compressed with
byte-for-byte SHA-256 verification before removing the uncompressed copies.

## Analysis

Python requires NumPy and SciPy; plotting additionally requires Matplotlib.
Use a virtual environment, then:

```sh
python tools/drift/analyze_reference.py tools/drift/reference/levels.json \
  --out tools/drift/measurements/levels.json
python tools/drift/analyze_reference.py tools/drift/reference/response.json \
  --out tools/drift/measurements/response.json
python tools/drift/analyze_reference.py tools/drift/reference/summing.json \
  --out tools/drift/measurements/summing.json
python -m unittest discover -s tools/drift -p test_analysis.py
```

Analyze `linear`, `linear-clean`, `ordering`, `sine-response`, and `highpass`
using the same command pattern. Validate the fixed Type-I linear candidate
against the independent sine-response captures with:

```sh
python tools/drift/validate_sine.py tools/drift/reference/sine-response.json \
  --report tools/drift/measurements/sine-response.json \
  --out tools/drift/measurements/sine-validation.json
```

Then run the component diagnostics:

```sh
python tools/drift/diagnose_reference.py tools/drift/reference \
  --measurements tools/drift/measurements \
  --out tools/drift/measurements/diagnostics.json
```

This tests hard-clip and tanh hypotheses with one training window and independent
note-order holdouts. The normalized clip uses pitch-level ratios measured in
the separate solo-oscillator batch; it does not fit each loud holdout. Quiet
filter comparisons reject harmonics less than 30 dB above a local background
estimate in either input level or path. This is a conservative diagnostic mask,
not a statistical confidence bound. Reports verify capture, ledger and set
hashes. These historical diagnostics identify component behavior; they do not
constitute an exact-match acceptance test for the production emulation.

Complex linear-section identification from the existing response batch:

```sh
python tools/drift/fit_linear.py tools/drift/reference \
  --report tools/drift/measurements/response.json \
  --out tools/drift/measurements/linear-section-fits.json
```

These per-case parameter fits compare 48/96 kHz candidate processing, including
a resonant high-pass section. Fitting and testing on the same capture identifies
candidates; it is not independent validation of a shipped model.

Continuous Type-II linear-law identification reserves intermediate resonance
steps and quieter controls for validation:

```sh
python tools/drift/fit_type2_linear.py tools/drift/reference \
  --reports tools/drift/measurements \
  --out tools/drift/measurements/type2-linear-law.json
```

`prototypes/feedback-root.lisp` contains a closed-form scalar feedback solver.
`prototypes/type1-step.lisp` uses explicit state arguments for the research
Type-I model. These are research versions; the production source contains its
own native-rate implementation. Check the
compiled root against independently bracketed numerical solutions with:

```sh
python tools/drift/check_feedback_root.py \
  --compiler crates/sequencer/tools/DGenLisp-macos-arm64 \
  --toolchain crates/sequencer/tools/dgen-toolchain \
  --out tools/drift/measurements/feedback-root-validation.json
```

The pinned macOS compiler archive currently omits its audit support files
(eseq-ywoh). The local measurement run supplies the unmodified audit script and
ABI lists from the exact pinned upstream revision through
`DGEN_BINARY_AUDIT_TOOL`; the binary audit is never disabled. The root check
also runs the generated-code fusion check. This is numerical verification,
not evidence of a complete native-audio match.

Validation rejects missing files, wrong sample rate/length/channels/encoding,
nonfinite or silent audio, and a set changed since ledger generation. Fundamental
frequency is estimated from the quiet bypass sine; harmonics are Hann-windowed
complex demodulations, through harmonic 256 or 20 kHz. No output gain or phase
correction is fitted. Mixed-oscillator cases report level statistics rather
than mislabeling their second fundamental as harmonic distortion.

Drift's bypassed Sine is not a mathematically pure sinusoid. Reported THD includes
its source harmonics; subtracting THD numbers does not isolate filter distortion.
Compare the same source and input gain through bypass and filtered paths, and
compare each filter to its own low-level response. At driven settings, harmonic
ratios describe nonlinear output spectra, not an LTI transfer function.

Duplicate cases quantify repeatability: retrigger and zero Drift do not imply
bit-identical output. Output-volume comparisons use the prescribed gain ratio,
with their residual assessed against duplicate variation. Characterization at
48 kHz and these settings is not a full-range stability or sample-rate proof.
