# ES Compressor — Punch / Level / Sustain

Builtin stereo effect implemented in `src/effects/es_compressor_dsp.lisp`,
registered by `src/effects/es_compressor.rs`, with its panel in
`content/ui/effects/builtin/es-compressor.lisp`. This replaces the earlier
single-mode effect; old parameter-index compatibility is deliberately not kept.

Three different controllers run continuously, even when unselected. Mode
changes crossfade their **gains**, not their state variables or coefficients.
They share parameter smoothing, audio alignment, drive, tone, and output trim.
No external assets, extracted calibration tables, FFTs, or oversampling.
Supported sample rates: **8–384 kHz**, fixed for an instance.

## Provenance and scope

This is not an SSL, Waves, Teletronix or Goodhertz emulation. No recovered
commercial-plugin code, tables, calibration constants, or recordings are used
by the implementation or public tests.

References:

1. D. Giannoulis, M. Massberg, J. D. Reiss, **Digital Dynamic Range Compressor
   Design—A Tutorial and Analysis**, JAES 60(6), 399–408, 2012. Foundation for
   the feedforward/log-domain structure, soft-knee gain computer, and timing
   terminology. Punch's coupled RC ladder below is our own circuit derivation,
   **not a transcription of an SSL timing network**.
2. J. Najnudel, R. Müller, T. Hélie, D. Roze, **Power-Balanced Dynamic Modeling
   of Vactrols: Application to a VTL5C3/2**, DAFx 2023,
   [paper](https://www.dafx.de/paper-archive/2023/DAFx23_paper_50.pdf).
   Level implements normalized carrier equations (4) and the conductivity law
   (7). Credit to these authors for the physical model. Our excitation circuit,
   normalized parameters, attenuator configuration and split implicit solver
   are described below. We do not reproduce the paper's identified device,
   full port-Hamiltonian circuit, or its discrete energy-balance guarantees.
   A Vactrol is not the LA-2A's T4 assembly.
3. V. Zavalishin, **The Art of VA Filter Design**: trapezoidal-integrator/TPT
   filter realizations. Tone uses standard first-order and state-variable
   low-pass sections, not a recovered spectral response.

The source is independently written from equations, not copied reference code.
Sustain preserves the existing ES controller's authored formulas. Private
listening references are not correctness oracles for the new controllers.
Published literature does not itself establish patent/license clearance.

## Punch — feedforward coupled timing

The input's linked peak is measured over the shared short lookahead window,
then converted to dB. With `u = amount/100`:

- Threshold: `-6 - 30*u` dBFS.
- Ratio: `1 + 7*u`.
- Knee width: 6 dB; quadratic transition into the usual above-threshold slope.

The target positive reduction `T` drives a passive two-capacitor RC ladder.
Capacitances are normalized; the first capacitor's charging resistor sets
`tau`, and the resistor connecting the capacitors sets `memory`:

```
da/dt = (T-a)/tau + (b-a)/memory
db/dt = (a-b)/memory
```

`tau` selects Attack when `T>a`, otherwise Release. `memory = Release/4`.
Both states influence the first node, which controls applied reduction.
The slow node retains charge across changes in target; this is not two
independent envelope followers mixed at their outputs.

For `d=dt/tau`, `c=dt/memory`, backward Euler gives:

```
a' = ((a+d*T)*(1+c) + c*b) / (1+d+2*c+d*c)
b' = (b+c*a') / (1+c)
gain = 10^(-a'/20)
```

The update has nonnegative weights, unity steady-state gain in the control
path, and no explicit-Euler sample-rate stability limit. Attack/Release are
component time constants, not a promise about the settling time of the whole
coupled network. Makeup is manual via Output. Amount 0 gives ratio 1:1.

## Level — feedback photocarrier dynamics

The detector observes the **previous sample of Level's own attenuated audio**,
before drive, tone, mode blending, Mix and Output. It therefore remains a true
feedback controller when Level is not selected. Both channels share the maximum
absolute feedback magnitude.

Let `n,p` be normalized electron/hole populations, with trap capacity 1.
The paper's Eq. (4), with a positive photogeneration rate `J`, becomes:

```
dn/dt = J - kn*(n-p)*n
dp/dt = J - kp*(1+p-n)*p
```

The valid region is `p>=0`, `0<=n-p<=1`. Conductivity is proportional to
`n + 0.2*p`; the normalized series-resistor/LDR divider gives
`gain = 1/(1 + u*(n + 0.2*p))`.

Our optical driver uses the same Amount-to-threshold mapping as Punch but not
its static gain curve. With feedback magnitude `v`, threshold amplitude `t`
and detector trim `gdet`:

```
e = max(0, v*gdet/t - 1)
light = (e/(1+e/8))^2
J = u * (1000/Attack_ms) * light
kn = 1000/Release_ms
kp = 4*kn
```

The driver has smooth finite headroom. All constants here are original
normalized tuning, not measured Vactrol/T4 or plugin data. Attack controls
excitation rate; Release controls recombination. **These controls also affect
the cell's operating point**, rather than acting as independent envelope poles.
The panel identifies this as optical excitation/recovery. Amount 0 removes
attenuation, even while stored carriers finish recovering.

Each sample adds equal generated charge, then solves electron and hole
recombination separately with backward Euler. The scalar quadratic roots use
`2*z/(b + sqrt(b*b + 4*k*z))`, avoiding subtraction cancellation. The DSP stores
`p` and trap occupancy `d=n-p`, not two nearly equal large populations. This
preserves nonnegative populations and bounded traps without state clipping or
iterative solvers. The split scheme is first-order accurate in time; it is not
an exact continuous-time solution. Recovery depends on the carrier populations,
not fixed weighted attack/release tables.

## Sustain — preserved ES dynamics

- Linked sliding peak with 0.25 ms lookahead and detector trim.
- Amount sweeps threshold from -36 to -66 dBFS, reduction slope from 0.6 to
  0.9, and the saturating reduction ceiling from 8 to 256 dB.
- Six-dB soft knee followed by `ceiling*(1-exp(-slope*over/ceiling))`.
- Target minus the mean of the previous 1 ms of reduction controls timing
  speedup and target overshoot. The finite history sum uses disjoint dyadic
  FIR blocks, not a drifting running subtraction.
- A -20 to -8 dBFS hot window enables faster waveform-following behavior.
- Automatic makeup is calibrated to the existing -23 dBFS reference level.
- The existing rational soft-knee shaper remains active at Drive 0.

These dynamics equations are unchanged. **Amount 0 is still not bypass in
Sustain**, unlike Punch/Level; use Mix 0 for dry. Its old gain-domain Mix law
and FFT coloration are intentionally replaced. To start near the previous
full-wet voicing, select Sustain, Amount 50, Attack 40 ms, Release 120 ms,
Tone 100, Drive 0 and Output -6 dB. The new filter is not spectrally identical.

## Shared audio path and controls

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| Mode | Punch / Level / Sustain | Punch | Manifest-backed discrete choice, supports p-locks. |
| Amount | 0–100 | 50 | Architecture-specific compression macro. |
| Tone | 0–100 | 0 | Flat to full dark voicing; independent of Mix. |
| Attack | 1–200 ms | 40 | Base timing / optical excitation; exponential knob travel. |
| Release | 20–2000 ms | 120 | Base timing / optical recombination; exponential knob travel. |
| Mix | 0–1 | 1 | Conventional linear dry/wet, identically aligned paths. |
| Drive | 0–24 dB | 0 | Shaper drive; 70% post-compensation. |
| Input | -24–24 dB | 0 | Before detector and dry/wet split. |
| Output | -48–18 dB | 0 | After dry/wet; manual makeup for Punch/Level. |
| Detector | -36–36 dB | 0 | Detector-only trim; does not directly amplify audio. |

Continuous controls use 10 ms exponential smoothing initialized directly to
the first value. The mode index is rounded/clamped, converted to three one-hot
weights, and smoothed with the same time constant. We normalize their sum;
Punch-to-Sustain never visits Level as an intermediate mode. Warm controllers
avoid startup gain bursts on mode changes.

The shared shaper is linear below magnitude 0.75, then approaches a 1.02
ceiling with a fourth-power rational curve. Punch/Level bypass it at Drive 0;
Drive 0–6 dB smoothly introduces it. Sustain retains the full shaper at unity
drive. Saturation occurs after gain blending.

Tone is an original +1.5 dB low shelf at 200 Hz followed by a fourth-order
Butterworth low-pass at 11.5 kHz. The low-pass corner is bounded to `0.45*fs`
at low rates. Its fixed-pole TPT filters run continuously; Tone smoothly blends
flat and filtered audio. This is not the former 12th-order FIR response.

Mix 0 passes the delayed input with Input and Output trims still applied.
With trims at zero it is sample-exact dry. No hidden final limiter is added:
parallel mix, drive, makeup, shelf boost and Output can exceed full scale.
There is no dedicated gain-reduction meter in this panel.

## Latency and cost

Declared latency is only `max(1, min(127, round(0.00025*fs)))` samples, common
to all modes and the dry path: **12 samples / 0.25 ms at 48 kHz**, versus
267 samples in the previous effect. Filter phase is not a compensatable delay.
Mode changes never change the PDC contract.

One local host-compiled measurement at 48 kHz: **13,388 float32 slots**
(~52 KiB) versus 548,930 (~2.09 MiB) previously. Private offline renders of
three breaks took about 1.4–1.5% of their audio duration, versus 6.0–6.6% for
the previous effect. These are wall times including the Python/host-services
adapter, not a realtime callback benchmark or a guarantee for other machines.
All three new controllers were running in these measurements.

## Validation

`effects::es_compressor::tests` uses only generated signals and checks:

- Manifest mode labels/default, fixed latency at multiple rates, bounded
  memory allocation and absence of FFT/assets.
- Punch against a double-precision RC reference.
- Level against the carrier equations in independent `n,p` coordinates,
  including physical state bounds at 8/48/192/384 kHz.
- Sustain against the previous controller equations.
- Distinct response trajectories, stereo linking, exact dry, affine parallel
  mix, and optional high-frequency attenuation.
- Warm, normalized mode transitions against independently rendered modes.
- Burst recovery and exact silence, including driven Sustain.
- Finite output and exact 31/128-frame automation partition invariance at
  8/44.1/48/96/192/384 kHz, with control extremes.

`state_values::tests::metal_seq_fx_es_compressor_layout_contains_knobs`
checks finite nonzero visible geometry and reactive mode selection. The durable
capture fixture is `ui/capture-fixtures/es-compressor-panel.lisp`.

Private audition recordings and reports stay under ignored `.local/`; they
are RMS-matched and latency-aligned, not published test fixtures. Their render
adapter additionally checks complete DSP memory for nonfinite state. Musical
tuning still requires listening approval; numerical tests are not a claim of
perceptual equivalence to any commercial plugin.
