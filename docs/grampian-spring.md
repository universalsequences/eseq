# Space Echo: independent Grampian spring

The King Tubby selector now uses a separate algorithm, not a retuning or an
average of the RE-201 tank. The RE-201 stationary fit is unchanged. This is an
algorithmic, Grampian-inspired model, not a verified circuit/mechanical replica.
Listening approval is tracked in `eseq-4ew3`.

## References and identification

`content/impulses/spring-references.json` pins the original Grampian recordings
by SHA-256 and records preparation/provenance limitations. The sweep recording
is the primary identification target. Filter 500 is a separate tonal cross-check:
aligned waveform correlation is about 0.92, so it is **not independent hardware
validation**. Neither is averaged into the other. Both are effectively mono.

The 18 Yamaha files are 0.966-second, predominantly direct-sound amp variations;
they are not 18 independent tanks. Park is another amp-spring candidate, and the
Lexicon plate is excluded. None is silently included in this fit. These recordings
are offline analysis inputs, not new convolution assets loaded by the app.

Use the original float-decoded recordings, preserving their start and fast HF
precursors. Peak trimming destroyed propagation timing in the previous pipeline.
Capture/interface latency and the original excitation level are not calibrated.

Measured primary first-packet times (ms):

| Frequency | Spring A | Spring B |
|---|---:|---:|
| 1 kHz | 16.25 | 24.0 |
| 3 kHz | 20.25 | 28.75 |
| 5 kHz | 35.5 | 46.5 |
| 5.5 kHz | 47.25 | 59.5 |
| 6 kHz | 71.0 | 90.0 |

`identify` follows two local-prominence peaks with monotone continuation, rather
than calculating a frequency-wise time centroid. Three groups of 40 conjugate
pole pairs per propagation leg approximate these tracks; group-delay fit RMS is
about 0.20/0.33 ms. This is a curated identifier, not a universal blind spring
identification algorithm. Near 6 kHz, multiple paths overlap: inspect its plots.

## DSP

`crates/sequencer/src/effects/spring/grampian.rs` owns its own flat state and
coefficient types. Each of two LF paths is:

```
input transducer HP + transition LP
  -> sum with returned wave
  -> forward delay + dispersive allpasses -> pickup
  -> return delay + dispersive allpasses
  -> boundary-scattering allpass -> frequency-dependent loss -> feedback
```

The first arrival and the round trip are distinct. A weak early reflection and a
stronger delayed boundary return produce the intermediate packets visible between
the reference's strong returns near 77/100 ms. Scattering is in the **return**,
not before the first pickup: it builds late density without washing out the first
splash. This topology is an explanatory approximation inferred from the IR, not
proof of the original tank's internal construction.

A separate short dispersive branch represents the fast HF precursor, with input
transducer resonance and its own loss/band limits. There is no undispersed 435 Hz
"shimmer" comb. The input filters are outside the feedback loops. Inside them,
loss magnitudes are bounded by unity and feedback gain is strictly less than one.

Conjugate-pole allpasses use pole frequency/bandwidth in Hz. There is no integer
stretch-factor switch and no phase-bank reinterpretation when moving tension.
Centered cubic Lagrange delay reads are passive at fixed settings and avoid the
large, sample-rate-dependent HF loss of linear delay interpolation. A regression
checks both this bound and measured band-decay agreement at 44.1/48/96 kHz.
Coefficients/delay capacities support 8–192 kHz; the fitting measurements target
normal host rates, not perceptual equivalence at 8 kHz.

Stereo uses the two pickups of the same tank. Width zero is genuinely mono;
changing width preserves the centered return. The RE-201 retains its existing
stereo tank arrangement. Type changes crossfade both real engines for 20 ms;
reactivating a dormant voice clears its old state rather than resurrecting a
frozen tail. Tension smoothing uses elapsed time rather than callback count.

The Grampian silence detector waits beyond a conservative delay/dispersion/pole
settling bound (including input filters), and below a -100 dBFS output-envelope
threshold, before clearing
state once and sleeping. Excitation below -160 dBFS does not keep it awake;
this matters for the preamp DC blocker's residuals. An empty output between
packets is not treated as an empty tank. Audible late returns re-arm the guard,
so it cannot expire merely because the original excitation was long ago.
Steady-state processing allocates no memory; state clearing is bounded
and happens on entering sleep or activating a previously dormant voice.

The tank itself is linear apart from that inaudible-tail optimization. Existing
Space Echo input electronics supply drive. A normalized impulse cannot identify
nonlinear electronics or amplitude-dependent mechanical behavior; no new "drip"
waveshaper is claimed to have been learned from these captures.

## Analysis and fitting

The old centroid objective could reward reversed chirps. The new objective uses:

- The actual compressed time-frequency magnitude surface over the first 300 ms.
- Energy-decay curves in six bands, with estimated stationary tail power removed.
- Band-energy spectrum from **80 Hz to 16 kHz**, including out-of-band leakage.

Decay comparisons stop above -30 dB and before two seconds. The tail-noise
estimate is a pragmatic measurement method, not a calibrated noise acquisition.
The low-frequency late residual is not fully reproduced; a long-lived component
near 246 Hz deserves particular caution when interpreting the lowest decay band.
The objective is not a perceptual quality score. Parameter accuracy and listening
quality are separate claims.

`grampian_tune.py` uses the exact stationary transfer function to accelerate fitting.
`verify` compares it with the Rust sample-by-sample implementation; it is not an
alternate runtime DSP engine. Compare/production auditions use Rust. The fitted
constants have a checked-in fixture and a test preventing it from drifting away
from the shipped defaults. Absolute return gain is calibrated separately against
the previous tank's 0.25-amplitude, three-second impulse RMS at 48 kHz.

## Measured result

Rust-rendered results against the primary capture (smaller is better):

| Metric | Previous voice | New Grampian |
|---|---:|---:|
| Early packet-surface error | 0.1024 | 0.0364 |
| Banded decay RMS error | 18.78 dB | 2.43 dB |
| 80 Hz–16 kHz band-energy spectral RMS error | 2.05 dB | 2.61 dB |

The filtered cross-check gives packet error 0.1048 → 0.0438 and decay error
18.64 → 2.54 dB, but spectral error 2.54 → 3.54 dB. This is a real tradeoff,
not across-the-board superiority. These banded metrics are **not** comparable
to the old fitter's published broadband-EDC residuals.

Removing return scattering increases packet error to 0.0499 and decay error
to 4.08 dB. Removing the precursor raises spectral error to 18.09 dB; removing
dispersion raises packet error to 0.0673 while barely changing spectrum.
That last ablation illustrates why matching tone alone did not produce a spring.
Analytical/Rust relative waveform L2 error is below 0.018% at 44.1/48/96 kHz.
Evidence is saved in `crates/sequencer/tests/fixtures/spring/grampian-evidence.json`.

Validation: 15 targeted Rust correctness tests, four Python analysis tests, and
the existing production CPU probe. On this Apple Silicon checkout, the release
probe at 48 kHz / 128-frame blocks measured Grampian echo+spring at **7.64% of
realtime**, versus 1.65% for the darker half-rate RE-201. This is an observed
single-core processing ratio, not a portable performance ceiling or app CPU meter.
The new preallocated tank state adds about 2.26 MiB per Space Echo node. It is
not a free extra voice: the accurate dispersion has a real CPU/memory cost.

The plot still shows a more regular HF precursor than the recording, and the
upper tail outlives the reference. Those are voicing/model limitations for the
listening review, not claims of exact hardware identification. The low-frequency
late mode also remains imperfect. See generated plots in `tuning_out/grampian-primary`.

## Reproduce and audition

Dependencies are declared in the Python scripts for `uv run`; ffmpeg is required.
No command below launches playback, a UI, or an audio device.

```sh
cargo build --release -p sequencer --bin spring_tune

uv run scripts/grampian_tune.py identify --out tuning_out/grampian-identify

uv run scripts/grampian_tune.py --bin target/release/spring_tune verify \
  --params crates/sequencer/tests/fixtures/spring/grampian-fit.json

uv run scripts/grampian_tune.py --bin target/release/spring_tune compare \
  --params crates/sequencer/tests/fixtures/spring/grampian-fit.json \
  --out tuning_out/grampian-primary

uv run scripts/grampian_tune.py --bin target/release/spring_tune compare \
  --params crates/sequencer/tests/fixtures/spring/grampian-fit.json \
  --reference grampian-filter-500 --out tuning_out/grampian-filtered

uv run scripts/grampian_tune.py ablate \
  --params crates/sequencer/tests/fixtures/spring/grampian-fit.json \
  --out tuning_out/grampian-ablations

uv run scripts/grampian_audition.py --out tuning_out/grampian-audition
```

Generated `tuning_out/grampian-*` plots, fits and WAVs are locally ignored; the
shipped parameter/evidence fixtures remain tracked separately.

The pack contains deterministic rim/snare/skank/bass/throw probes, linear-reference
convolution, the old and new isolated linear responses, and actual production
Space Echo renders (mono, wide, driven, echo + spring). These are synthetic musical
probes, not emulations of specific instruments. Pass `--input your-dry.wav` to use
real material. `levels.json` records normalization and raw levels; mono and wide
share a gain. Do not mistake output normalization for measured nonlinear fidelity.

For direct production renders:

```sh
target/release/spring_tune --voice grampian --space-echo \
  --input dry-48k.wav --amp 1 --sr 48000 --seconds 4 \
  --host-settings settings.json --wav wet.wav --benchmark 5
```

`--seconds` adds tail silence with `--input`; otherwise it sets IR duration.
`settings.json` accepts the fields of `SpaceEchoRenderSettings`, for example
`{"mode":7,"width":0.7,"input_db":12,"intensity":0.65}`. Modes are zero-based:
11 is spring only. CLI `--sr` and `--voice` set the render clock and voice;
other production controls, including tension, belong in the settings JSON.
Noise and wow are disabled for deterministic auditions, but the actual preamp,
tape feedback, tone, spring routing, and output mix are exercised.

`fit --params <seed> --geometry <identified.json>` refines the primary voice,
keeping identified dispersion fixed and limiting return delays to the measured
neighborhood. `--precursor-only` freezes the two main springs. It never rewrites
Rust source automatically. Inspect the resulting spectrum/packets, run `verify`,
then explicitly update the constants and fixture before production comparisons.
