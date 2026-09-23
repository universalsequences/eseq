# ES Compressor — Punch / Level / Sustain

Builtin stereo effect implemented in `src/effects/es_compressor_dsp.lisp`,
registered by `src/effects/es_compressor.rs`, with its panel in
`content/ui/effects/builtin/es-compressor.lisp`.

Punch and Level are independently authored **behavioral models fitted to
black-box audio measurements**. They replace the earlier coupled-RC Punch and
normalized-photocarrier Level designs. Mathematical validity of those designs
did not establish the musical behavior the owner wanted. Sustain's controller
is unchanged.

All three paths run continuously. Mode changes crossfade aligned **audio**, not
controller state; Level has its own audio conditioning, so a gain-only crossfade
would be incorrect. No external assets, calibration files, FFTs or oversampling
are needed. Supported rates are **8–384 kHz**, fixed per instance.

## Identification boundary and scope

The owner selected the local SSL-style and CLA-2A-style **study effects**, not
commercial plugin binaries, as sealed behavioral references. They were compiled
and exercised through audio input/output and ordinary public controls. Reference
source, recovered coefficients, calibration tables and controller state were
not used as identification inputs. Opaque hashes identify the reference versions.
Study READMEs had been read before establishing this boundary: this is **not a
clean-room legal certification**, a claim of patent/license clearance, or a
claim that the studies themselves have independent provenance.

The new DSP uses standard independently written building blocks:

- Quadratic soft knees, following the general compressor-design treatment in
  Giannoulis, Massberg and Reiss, *Digital Dynamic Range Compressor Design—A
  Tutorial and Analysis*, JAES 60(6), 2012.
- Exponential relaxation, exposure memory and convex combinations of linear
  gains. The Level model is not a simulation of a T4, Vactrol or tube circuit.
- First-order/TPT state-variable filters, as described by Zavalishin,
  *The Art of VA Filter Design*.

The fitted numbers describe **our model**, not extracted reference internals.
No private audio or reference implementation is included in public tests.

### Measurement and model selection

The private baseline contains steady 1 kHz curves, 20/200/2000 ms bursts with a
quiet recovery pilot, frequency sensitivity and asymmetric-stereo probes.
Additional Level bursts span -12 to +12 dBFS peak at two public compression
settings. Audio-path sweeps separate nominal gain/filtering from compression.
Fits use analytic soft knees, detector filter magnitudes and numerical relaxation
models. Candidate selection also uses actual `breaks`-tagged library material:

- Four development excerpts, at native input level and external -12/+12 dB trim.
- Four other excerpts reserved from fitting, tested only after candidate freeze.
- Two complete excerpt passes precondition each controller; the third is measured.
- Raw float outputs retain level. Listening copies are latency-aligned and
  RMS-matched with one common headroom scaling per comparison, not LUFS-matched.

The final identification candidate is `fit-v1/candidate-05.lisp` under the
ignored `.local/es-compressor-multimode/` directory. Production removes an unused
Punch acceleration argument and uses the precision-preserving split accumulation
described below. It does not import private scripts or recordings at runtime.

On the 12 reserved musical cases per mode (four excerpts × three input levels),
median gain-matched waveform NRMSE fell from **0.1333 to 0.0291 for Punch**, and
**0.1445 to 0.1181 for Level**. Median 5 ms-envelope RMS error fell from
**1.467 to 0.418 dB**, and **1.285 to 0.632 dB**, respectively. These are error
metrics, **not perceptual similarity percentages**. All reserved cases improved
in waveform error, but one Level case worsened in envelope error. One development
Level case also slightly worsened in waveform error. Reports retain those results.

Level remains an approximation, particularly on transients/hot material. Its
small measured reference noise and nonlinear harmonic components are deliberately
not reproduced; Drive remains the explicit nonlinear-color control. Identification
was at 48 kHz. Cross-rate safety/numerical tests are not cross-rate reference
fidelity certification. Listening approval is still necessary.

## Shared gain computer and detectors

Let `u = Amount/100`. For detector level `L`, threshold `H`, slope `s` and knee
width `K`, the nonnegative target reduction in dB is:

```
e = L-H
z = clamp(e+K/2, 0, K)
T = s * (z²/(2*K) + max(0, e-K/2))
```

Punch and Level filter each detector channel, link on the larger instantaneous
squared value, and smooth that power with a 0.5 ms pole. Detector dB is
`10*log10(2*power) + Detector`: the factor two makes a steady sine's nominal
level peak-equivalent. It is not a promise of peak detection for arbitrary audio.
Sustain keeps its original sliding-maximum detector.

Fixed detector filters use TPT poles. Shelf/low-pass responses are analytically
normalized at 1 kHz using the actual sample rate, not a 48 kHz-only correction.
Corners approaching Nyquist are bounded to the documented fractions in source.
Detector filters do not directly EQ the audible Punch path.

## Punch — broad knee and short recovery memory

```
H = -9.9652 - 43.6853*u + 16.487*u²
s = 0.80984 * (1-exp(-3.70731*u^1.38568))
K = 19.5 dB
```

Threshold falls and slope rises monotonically over the control range. Amount 0
removes reduction. The detector has a gentle rising shelf (about 3 dB) plus
calibration trim. This avoids transferring Level's strong bass/treble sensitivity
to Punch.

A reservoir `m` follows target `T`, charging with `1.4787*Release` and discharging
with `2.4024*Release`. The applied reduction `r` follows:

```
if T > r:
    aim = T
    tau = 1.0475*Attack
else:
    aim = min(r, T + 0.15335*max(0, m-T))
    tau = 0.8444*Release
r += (1-exp(-dt/tau)) * (aim-r)
gain = 10^(-r/20)
```

The reservoir retains exposure across hits; the release aim cannot make reduction
increase during recovery. This is a bounded feedforward behavioral controller,
not the former passive RC ladder. Makeup is manual via Output. At Amount 50,
Attack 40 and Release 120, a -24 dBFS-peak 1 kHz tone receives about 2.75 dB
reduction, versus effectively none in the prior Punch implementation.

## Level — two linear-gain relaxation populations

The detector includes a bass shelf, treble shelf and two-pole low-pass. The
combined normalized response, rather than any individual shelf gain, is the
intended detector weighting. At default settings and -12 dBFS peak, the compiled
candidate's attenuation relative to nominal gain is approximately 4.01 dB at
60 Hz, 1.31 dB at 1 kHz and 6.73 dB at 8 kHz. It is intentionally not flat.

```
H = 3.7361 - 33.7184*u
s = 0.75 * min(1, 4*u)
K = 7 dB
q = 10^(-T/20)
a = 1 + 17.0739 / (1 + (T/4.26775)^7.53749)
```

Two positive gain states start at unity and independently follow `q`:

| Population | Attack time constant | Release time constant | Mix weight |
|---|---:|---:|---:|
| Fast | `0.0817833*Attack*a` | `2.30597*Release` | 0.815406 |
| Slow | `0.901311*Attack*a` | `43.3357*Release` | 0.184594 |

Attack is selected when the target gain is lower than the population's current
gain. Each state uses `state += (1-exp(-dt/tau))*(q-state)`; attenuation is their
weighted sum. Near-threshold onset is gentle; larger reductions charge faster.
The slow population charges less on short hits than on sustained passages,
creating exposure-dependent recovery. Combining **linear gains**, not dB
reductions, also gives deep compression a quicker initial recovery without an
ad hoc dB slew ceiling or saturating dB-memory law.

The audible Level input has a 4 Hz DC blocker, followed by the shared alignment
delay and attenuation. Its output has a gentle 0.65 dB dip around 11.5 kHz.
These are independent approximations of measured audio conditioning, not recovered
filter coefficients. Both channels share attenuation, with identical audio filters.

**Level has fixed +8.5 dB nominal gain**, matching the chosen reference operating
point. Output is an additional trim; -8.5 dB cancels that nominal gain. It does not
vary secretly with Amount. Amount 0 removes compression but leaves Level's gain
and conditioning active. Use Mix 0 for dry. Attack/Release scale the population
time constants; their displayed values are not literal times to a specified
percentage of final reduction.

## Numerical behavior

For long time constants at high rates, computing `1-exp(-x)` directly in float32
loses precision. `follower-step` uses `x-x²/2+x³/6-x⁴/24` below `x=0.01` and the
ordinary expression elsewhere. Truncation error of the small-x branch is below
`1e-12` before float rounding.

The shipped compiler uses fast-math: it demonstrably removes ordinary Kahan
compensation. The DSP therefore uses an **explicit integer/fraction split
accumulator**, not a cancellation expression or a compiler-flag override. For
scale `S=16384`, state is represented as `(whole+fraction)/S`:

```
units = fraction + S * alpha * (target - previous)
carry = round(units)
whole_next = whole + carry
fraction_next = units - carry
next = (whole_next + fraction_next) / S
```

Whole parts are exact float32 integers; residual increments accumulate near zero
and carry through the explicit rounding operation. BOTH parts are reconstructed
for the next sample and audible output: this is not a 1/16384 gain quantizer.
Level gains lie in (0,1]; finite Punch detector targets are below 512 dB, so
whole parts remain below 2^23. The representation has ample headroom without
clamping the authored curve. Its semantics do not depend on disabling algebraic
reassociation. The actual host-compiled DSP is tested against f64 integration,
not merely a handwritten model of this accumulator.

Level's longest release is about 86.7 seconds at Release 2000; this is exercised
at 384 kHz. Gains are stored directly, not as `1-gain`, avoiding cancellation
under deep compression. Updates are convex relaxations with finite positive time
constants; there is no iterative solver, reduced-rate envelope clock, state
reset on mode change, or hidden output limiter.

## Sustain — preserved ES dynamics

- Linked sliding peak with 0.25 ms lookahead and detector trim.
- Amount sweeps threshold from -36 to -66 dBFS, slope from 0.6 to 0.9, and the
  saturating reduction ceiling from 8 to 256 dB.
- Six-dB soft knee followed by `ceiling*(1-exp(-slope*over/ceiling))`.
- Target minus the mean of the previous 1 ms of reduction controls timing
  speedup and overshoot; history uses the original disjoint dyadic FIR blocks.
- The -20 to -8 dBFS hot window enables faster waveform-following behavior.
- Automatic makeup remains calibrated to the original -23 dBFS reference.
- The rational soft-knee shaper remains active at Drive 0.

These controller equations are unchanged. Amount 0 is not bypass in Sustain.

## Controls and shared audio path

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| Mode | Punch / Level / Sustain | Punch | Manifest-backed discrete choice, supports p-locks. |
| Amount | 0–100 | 50 | Mode-specific threshold/curve macro. |
| Tone | 0–100 | 0 | Common wet-path flat-to-dark blend; not a detector EQ. |
| Attack | 1–200 ms | 40 | Mode-specific onset time scale; exponential knob travel. |
| Release | 20–2000 ms | 120 | Mode-specific recovery time scale; exponential knob travel. |
| Mix | 0–1 | 1 | Linear dry/wet with identical declared alignment. |
| Drive | 0–24 dB | 0 | Shared nonlinear shaper drive; 70% post-compensation. |
| Input | -24–24 dB | 0 | Before all detectors and the dry/wet split. |
| Output | -48–18 dB | 0 | Final trim; Punch makeup is manual, Level includes nominal gain. |
| Detector | -36–36 dB | 0 | Detector-only trim, no direct audio gain. |

Continuous controls retain 10 ms dezippering initialized to the first value.
Three one-hot mode weights are smoothed and normalized; Punch-to-Sustain does
not detour through Level. All controller and audio-filter histories stay warm.

The shared shaper remains linear below magnitude 0.75, then approaches a 1.02
ceiling with a fourth-power rational curve. Punch/Level bypass it at Drive 0;
Drive 0–6 dB introduces it smoothly. Tone remains the original +1.5 dB low shelf
at 200 Hz followed by a fourth-order Butterworth low-pass at 11.5 kHz, blended
on the wet side. Level's native conditioning remains present at Tone 0.

Mix 0 passes the delayed input with Input/Output trims still applied. With trims
at zero it is sample-exact dry in every mode. Wet signals, makeup, Drive and
Output can exceed full scale. There is still no dedicated gain-reduction meter.

Declared latency is unchanged: `max(1, min(127, round(0.00025*fs)))` samples,
**12 samples at 48 kHz**, common to all modes and dry. Filter phase is not a
compensatable fixed delay. No performance claim is inferred from offline render
wall times; all three modes continue running.

## Regression coverage

`effects::es_compressor::tests` uses generated signals, never private recordings:

- Manifest, mode/default contract, bounded memory and declared latency.
- Punch's reservoir and Level's gain populations against independent f64
  integration, including extreme timing at 8/48/384 kHz.
- Useful compression depth, Amount and Detector engagement, and Level's distinct
  bass/treble sensitivity with Tone off.
- Exposure-dependent recovery and effective Attack/Release controls.
- Preserved Sustain equations, stereo linking, exact dry and affine parallel mix.
- Warm mode changes, output-trim range, burst recovery, silence, optional Tone.
- Finite output and exact 31/128-frame automation partitioning across
  8/44.1/48/96/192/384 kHz and control extremes.

`state_values::tests::metal_seq_fx_es_compressor_layout_contains_knobs` protects
finite nonzero control geometry and reactive mode selection. The production
capture fixture is `ui/capture-fixtures/es-compressor-panel.lisp`.
