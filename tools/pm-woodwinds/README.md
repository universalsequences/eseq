# Factory physical woodwinds

PM Flute keeps the original jet/body model and its default sound. The `flutter`
group exposes the two breath oscillations: `rate` (2 Hz), `drift_hz` (0.4 Hz),
`depth` (0.30 maximum breath reduction), and `floor` (0.02 minimum reduction).
Set depth to zero to disable both. Floor is limited to depth, so crossed knobs
cannot invert the modulation. All four are host modulation destinations.

Both instruments now have custom rose-pink physical-model panels: eight
persistent timbre knobs, a contextual center display, and section selectors.
The flute exposes Jet, Loop (including the editable breath contour), Body,
and Flutter. The sax separates Original, Reed, Bore, Motion, Body, and Amp.
Controls use the host's scoped parameter, modulation and parameter-lock paths.
The graphics show named mechanisms or nominal responses, not live audio.

Flute output gain is now connected, with unity at its original 0.5 default.
Its embouchure range now includes the original 0.7 default and follows the
jet model's existing 0.1–0.9 limits. A misspelled body-frequency default is
corrected to the authored 1500 Hz; it previously initialized to an out-of-range
0 Hz while the knob displayed its 50 Hz minimum. Default body mix remains
zero, so this correction leaves the original default audio unchanged.

PM Saxophone retains the original reed loop at `bore.acoustic = 0`; the default
and **Original Reed** preset select it. Its low-register behavior, including
roughly octave-down oscillation, is intentional. The old `stiffness`,
`brightness`, and `brnoise` controls still address this loop. The existing gain
parameter now controls output level, with unity at its original default 0.5.

Turning `acoustic` toward one blends in a separately tuned split waveguide.
This is a hybrid reed/string waveguide approximation, **not a geometrically
exact conical saxophone or an identified replica of the recordings**. Both
loops remain live while blending. The acoustic voice has velocity-scaled
output and an independently adjustable velocity-to-pressure response.

| Group | Controls | Role |
| --- | --- | --- |
| bore | acoustic, blow_pos, damping, bell_hz | Original/acoustic blend, excitation position, reflection loss, output lowpass |
| reed | pressure, reed, closure, air, vel_blow | Blowing level, reflection slope, reed closure offset, filtered turbulence, velocity response |
| color | body, body_hz, body_q | Passive output formant blend, center, width; outside the feedback loop |
| expression | growl, growl_hz | Audio-rate breath modulation for rough/alien tones |
| expression | vib_cent, vib_hz, vib_wait, vib_air | Pitch vibrato in cents, rate, onset delay in milliseconds, breath vibrato |

The physical controls have wide ranges; some combinations choke the reed,
squeak, or overblow. Acoustic presets are starting points rather than a
restriction on that range. `acoustic` uses a 10 ms transition and the principal
reed/bore controls use 5–8 ms smoothing. Vibrato fades in over 150 ms after its
onset delay and restarts that delay on a note trigger. Growl is independent.
The reed reflection is bounded and the lossy termination remains passive.
The bell/body filters are outside the physical feedback path.

## Reference material and presets

`reference-analysis.json` records hashes and measurements of the user-supplied
WAVs in `samples-to-analyze`; no recordings are included in the instrument or
redistributed here. `analyze_references.py` regenerates that report (numpy,
scipy, soundfile). The sustained notes are approximately 183.44 Hz and
369.53 Hz. Their shallow vibrato measures about 6–8 cents at 4.6–5.3 Hz.
The short moving clip is not treated as a steady spectral target.

- **Original Reed** — the existing synth, unchanged at default gain.
- **Alto Soft** — a restrained, second-harmonic-forward starting point.
- **Alto Full** — fuller low-register reed tone, guided by the louder reference.
- **Reed Growl** — stronger excitation and audio-rate breath modulation.
- **Split Reed Bass** — blends the original low voice with the split bore.
- **Air Column** — breathier, slower articulation and deeper vibrato.

Spectral agreement is measured on rendered audio, not a proof of perceived
realism. The waveguide does not simulate a full tone-hole lattice, reed mass,
vocal tract, or the player's detailed articulation. Listening remains the
acceptance test for the sax likeness. The original model's high notes can die
away; that behavior is preserved rather than folded into the acoustic preset
checks.

## Validation

Use the pinned compiler and stage installed by the repository fetch scripts.
The Python verifier needs numpy and reuses `tools/audition/audition.py`.
It compiles and drives the actual DGen DSP and checks internal state as well as
audio (the output ABI can sanitize NaNs, so output-only checks are insufficient).
Baseline source fixtures record the user-authored instruments before this work.

From the repository root:

```sh
cargo nextest --version
ESEQ_PM_VERIFY_DIR="$PWD/tools/pm-woodwinds/output/roundtrip" \
  cargo nextest run -p eseqlisp --test pm_woodwinds \
  -E 'test(=factory_woodwind_sidecars_preserve_executable_controls)'
PYTHONDONTWRITEBYTECODE=1 python3 tools/pm-woodwinds/verify.py \
  --roundtrip tools/pm-woodwinds/output/roundtrip
# For only the saved-graph and actual modulation-input checks:
PYTHONDONTWRITEBYTECODE=1 python3 tools/pm-woodwinds/verify.py \
  --roundtrip tools/pm-woodwinds/output/roundtrip --roundtrip-only
cargo run -p sequencer --bin instrument_probe -- \
  'factory:Physical Models/PM Saxophone' --preset 'Alto Soft' \
  --midi-note 66 --sample-rate 48000 --frames 144000 --gate-frames 96000 \
  --min-peak 0.01 --min-rms 0.001 --json
cargo run -p sequencer --bin instrument_probe -- \
  'factory:Physical Models/PM Flute' --midi-note 66 --sample-rate 48000 \
  --frames 144000 --gate-frames 96000 --min-peak 0.01 --min-rms 0.001 --json
```

The verifier covers original-sound equivalence, flute modulation disable/range,
preset sustain/release, individual control extremes at five pitches, joint
feedback corners, p-lock-like automation/retriggers, acoustic intonation at
44.1/48/96 kHz, velocity response, block partitioning, and patch-save audio
roundtrips. It runs the compiler fusion guard on every build and writes a
source/compiler-hashed JSON report plus level-adjusted listening previews.
No full Rust package/workspace suite is needed for this content change.

For the custom panels and the flute control fixes:

```sh
cargo nextest run -p sequencer --bin metal_seq -E 'test(/pm_woodwind_ui_tests::/)'
PYTHONDONTWRITEBYTECODE=1 python3 tools/pm-woodwinds/verify.py --flute-controls-only
target/debug/metal_seq capture \
  --script crates/sequencer/ui/capture-fixtures/pm-woodwinds-motion.lisp \
  --buffer fx --track 1 --width 1800 --height 600 --out /tmp/pm-flute-motion.png
```

The layout tests compile the real manifests, visit every page, and check
visible geometry, parameter coverage, reactive graphic inputs, section
callbacks, parameter edits and envelope batches with and without p-locks.
The flute control render report is `flute-ui-validation.json`; it covers
gain scaling/muting, embouchure limits, and retained default audio at
44.1, 48 and 96 kHz. Compared with the original static flutter source,
the oscillator expressions accumulate small floating-point differences in
the recursive jet; the test bounds both peak and RMS error.

The authored v3 sidecars are synchronized with the executable source. The old
sidecars carried fixed 44100 values where the source used `samplerate`;
regenerating removes that stale graph state. Only local macro graphs are stored;
external library definitions are resolved on load. Existing layout coordinates
are retained where the node identity survives projection.

Visual check (macOS):

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/pm-woodwinds.lisp \
  --buffer fx --track 0 --width 2200 --height 620 \
  --out /tmp/pm-saxophone.png
```

Track 1 captures the flute. Both use the normal generated parameter panels.

## Model references

The split-waveguide topology follows the published hybrid approach described
by [Cook and Scavone's Saxofony model](https://github.com/thestk/stk/blob/master/include/Saxofony.h).
This implementation adds explicit loss-phase compensation, separate expression
controls and output coloration; it does not claim that the hybrid is a literal
conical bore. The model is authored here in DGenLisp rather than linked to STK.

[Smith's single-reed theory](https://www.dsprelated.com/freebooks/pasp/Single_Reed_Theory.html)
explains the pressure-dependent reflection model. The distinction between
saxophone and clarinet harmonic support is described in
[UNSW's saxophone acoustics](https://www.phys.unsw.edu.au/jw/saxacoustics.html).

Measured on Apple Silicon macOS: 358 DSP renders and 12 saved-graph/modulation
renders passed. Acoustic preset tuning stayed within 1.66 cents at the tested
110/185/370/740 Hz notes across 44.1/48/96 kHz. Original sax default audio was
bit-identical; flute default max sample error was 5.21e-6. See `validation.json`
for source/compiler hashes and the scope of these claims.
