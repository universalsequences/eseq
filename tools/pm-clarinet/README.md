# PM Clarinet

Factory instrument: `Physical Models/PM Clarinet`, beside PM Flute and PM
Saxophone. It synthesizes a cylindrical single-reed bore in DGenLisp. The
recording informs the voicing; the instrument contains no sample playback.

The default **Reference Reed** has the supplied note's strong third harmonic
and hollow spectrum, with vibrato off. The pink physical-model panel exposes
25 parameters across Reed, Bore, Body, Motion and Amp. Eight primary knobs
stay visible; the center provides contextual controls, reactive mechanism
drawings, and an editable breath envelope. All controls use the normal host
parameter and parameter-lock paths; 19 are modulation destinations.

| Preset | Character |
| --- | --- |
| Reference Reed | The measured D4 reference guides the default voicing |
| Chalumeau | Darker low register, broader body response, softer attack |
| Soft Vibrato | Natural voicing with delayed 7-cent pitch motion |
| Breathy Wood | More turbulence and a broader body resonance |
| Reed Bass | Dispersive low bore, fast attack and low-rate growl |
| Warped Glass | Strong dispersion, harder blowing and audio-rate breath motion |

For natural sounds, start with Reference Reed and Warp at zero. Pressure,
Stiffness, Closure and Reed curve change how the reed starts, saturates and
closes. Body mix/Hz/Q shape the output resonance; Damping Hz controls losses
inside the bore, while Bell Hz rolls off the output. These controls interact:
their wide ranges intentionally allow silence, overblowing and rough tones.
For alien sounds, raise Warp and Growl, then move the reed controls. Vibrato
depth is in cents, with separate rate, delay and breath-amplitude depth.

## Physical model

The pressure-wave loop follows the cylindrical single-reed formulation in
[Julius O. Smith's Digital Waveguide Single-Reed Implementation](https://dsprelated.com/freebooks/pasp/Digital_Waveguide_Single_Reed_Implementation.html)
and [Single-Reed Theory](https://www.dsprelated.com/freebooks/pasp/Single_Reed_Theory.html).
The half-period loop and explicit filter-phase correction are also documented
in the [STK Clarinet implementation](https://github.com/thestk/stk/blob/master/src/Clarinet.cpp).
This is an independent DGenLisp implementation, with controllable reflection
curvature, dispersion, loss, expression and output coloration.

For mouth pressure `m` and the returning pressure wave `r`:

```text
delta = m - r
linear = clamp(closure + stiffness * delta, -1, 1)
reflection = sign(linear) * abs(linear)^curve
outgoing = m - delta * reflection
```

An inverting termination produces the cylindrical closed/open tube's odd
resonances. A one-pole lowpass, reflection loss and a first-order allpass act
inside the feedback loop. Warp sets the allpass coefficient to `-warp`, with
its magnitude limited below one. At zero Warp the allpass is exactly a
one-sample delay. The fractional bore delay subtracts the lowpass phase,
allpass phase, and outgoing-history sample at the requested fundamental from
the half-period. Thus damping and dispersion do not require a hidden pitch
offset. Warp still changes higher partial spacing and can change the nonlinear
oscillation regime.

The reed reflection stays within [-1, 1]; the static termination filters are
passive. Principal reed/bore parameters have 5–8 ms smoothing. The pressure
envelope, velocity-to-breath response, filtered turbulence and growl drive the
mouth. The delay-time dependency contains no amplitude envelope. Vibrato
fades in over 150 ms after its delay, which restarts on a trigger. A 20 Hz DC
block, bell lowpass and Q-normalized body bandpass mix sit outside the bore;
velocity and gain scale the final output.

This is a simplified waveguide, not a full tone-hole lattice or a model of
reed mass, vocal tract and player articulation. One sustained note cannot
identify those systems. The spectral fit is useful evidence for a voicing,
not proof of perceptual identity with a real clarinet. Extreme settings can
produce intentional roughness and aliasing; the reed nonlinearity is not
oversampled. No additional host or compiler workaround is introduced.

## Reference analysis and voicing

The user supplied
`samples-to-analyze/249113__clarinet_pablo_proj__overall-quality-of-single-note-clarinet-d4.wav`.
Its hash and measurements are in `reference-analysis.json`; audio remains
outside factory content.

- Mono, 48 kHz, 6.483 seconds; sustained analysis window 1–5.5 seconds.
- Fundamental 295.609 Hz: approximately D4, 11.4 cents above equal temperament.
- The third harmonic is strongest; odd/even harmonic power ratio is 17.94 dB.
- Measured 10–90% amplitude rise is about 61.7 ms.
- Pitch variation in the 3–9 Hz band is only about 0.44 cents peak-equivalent.
  This does not establish intentional vibrato, so the default adds none.

`analyze.py` tracks the clear third harmonic and measures 16 harmonic bands.
`fit.py` runs a bounded, seeded differential-evolution search over ten reed,
bore and color controls using the actual compiled DSP. It writes an experiment
report, not factory files. The recorded 1,501-render search reduced the
weighted harmonic dB mean-square error from 142.68 to 10.58 before reaching
its iteration budget; it did not claim optimizer convergence. The reviewed,
rounded factory defaults measure 10.60 on the final validation window.
`voicing-search.json` preserves the search result. Preset gains are balanced
separately from this normalized spectral comparison.

## Reproduce the checks

Use the pinned DGenLisp compiler and hermetic toolchain from the repository
fetch scripts. Python needs numpy, scipy and soundfile; use an isolated venv.
The checked-in reports were produced on Apple Silicon macOS. The verifier
selects the appropriate pinned compiler on Linux, but this task did not
establish a Linux baseline.

From the repository root:

```sh
python3 -m venv /tmp/eseq-pm-clarinet-env
/tmp/eseq-pm-clarinet-env/bin/python -m pip install numpy scipy soundfile
PYTHONDONTWRITEBYTECODE=1 /tmp/eseq-pm-clarinet-env/bin/python tools/pm-clarinet/analyze.py

cargo nextest --version
ESEQ_PM_VERIFY_DIR="$PWD/tools/pm-clarinet/output/roundtrip" \
  cargo nextest run -p eseqlisp --test pm_woodwinds \
  -E 'test(=factory_woodwind_sidecars_preserve_executable_controls)'
PYTHONDONTWRITEBYTECODE=1 /tmp/eseq-pm-clarinet-env/bin/python tools/pm-clarinet/verify.py \
  --roundtrip tools/pm-clarinet/output/roundtrip
cargo run -p sequencer --bin instrument_probe -- \
  'factory:Physical Models/PM Clarinet' --preset 'Reference Reed' \
  --midi-note 62 --sample-rate 48000 --frames 144000 --gate-frames 96000 \
  --min-peak 0.01 --min-rms 0.001 --json
cargo nextest run -p sequencer --bin metal_seq \
  -E 'test(=state_values::tests::pm_woodwind_ui_tests::clarinet_surface_controls_and_pages)'
```

`validation.json` records source, preset, reference and compiler hashes with
344 passing audio/state renders. It covers all six presets over five pitches,
individual control extremes, 64 joint reed/bore corners, automation,
retriggers, velocity, gain, host modulation, and source/graph-save equivalence.
Finite internal state is checked as well as output: output sanitation alone
could conceal invalid DSP state. The compiler fusion guard passed on all
seven unique builds.

Natural-preset tuning stayed within 2.18 cents at 110, 220, 295.609, 587.33,
1174.66 and 1760 Hz across 44.1, 48 and 96 kHz. Maximum observed peak across
the render checks was 0.672. Block sizes 32/64/128/256 produced identical
audio, including noise and expression. All six presets were bit-identical
after graph save. These are bounds for the tested cases, not an exhaustive
proof for arbitrary modulation. Listening exports are under the ignored
`output/` directory and preserve relative preset levels.

The layout test visits all five pages and checks finite, visible geometry,
parameter coverage, reactive shader inputs, section selection, ordinary edits,
parameter locks, and envelope batches. The authored v3 patch sidecar contains
the three local macro graphs, synchronized with executable source.

Production Metal capture, without opening an audio device:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/pm-clarinet.lisp \
  --buffer fx --track 0 --width 1800 --height 600 --out /tmp/pm-clarinet.png
```

The adjacent `pm-clarinet-bore`, `pm-clarinet-body`, `pm-clarinet-motion` and
`pm-clarinet-contour` fixtures select the other pages. All five captures were
inspected for text clipping, spacing and display content. Display curves are
labeled mechanism or nominal-response views, not live audio measurements.
