# PM Crash, PM Ride and PM Hi-Hat

These factory instruments use one parameterized physical synthesis engine per
type. Recordings supply measured losses, radiation weights and a small set of
resolved resonances. Playback runs the model; it does not read recordings,
recorded phase, spectral frames, or sampled amplitude envelopes.

The reference folder is `samples-to-analyze/Acoustic Cymbals Vol.1 by Donit`.
The analysis covers 23 crashes, 29 rides and 20 hi-hat articulations. Eight other
muted/scraped/percussive recordings are outside these three types. See
[ATTRIBUTION.md](ATTRIBUTION.md) and the supplied [source note](source-readme.txt).
The source folder and generated audio must remain excluded from commits.

## Physical model

This is a reduced, statistical waveguide model of metal vibration, with a
separate resolved-mode component. It does not recover a unique cymbal geometry
from audio, simulate a full nonlinear shell, or reproduce every recording
exactly. Low-order resonances alone produced an unconvincing bell-like hi-hat;
the diffuse body and contact dynamics are essential.

Six frequency regions each contain three coupled delay paths. Their scattering
matrix is orthogonal, with propagation losses derived from positive decay
rates. Fractional propagation uses a first-order allpass, avoiding the unwanted
high-frequency loss of linear delay interpolation. Eighteen radiation filters
represent the broad spectral envelope, while eight damped complex modes retain
the strongest individually resolved resonances. Production radiation and modes
use tensor loops; the calibration basis exposes their outputs separately.

A finite stick-contact force excites the waveguide body. A short compression
impulse excites the resolved modes. There is no independently enveloped noise
tail. Hardness changes the contact duration and force bandwidth; size changes
propagation times and modal frequencies. Touch adds loss to the vibrating body.
Gate release leaves a struck cymbal ringing.

The hi-hat adds energy-normalized relative shell motion and a unilateral
contact junction. A normalized projection couples the contact to all six
regions. The impact matrix has eigenvalues 1 and minus the restitution
coefficient, so collisions cannot increase stored wave energy. Closing also
removes displacement beyond the contact boundary. Unresolved microcontacts
vary an orthogonal reflection matrix using centered, variance-normalized noise.
That randomness changes scattering, rather than injecting sound into a ringing
body. It is a statistical contact approximation, not a reconstructed two-shell
finite-element model.

Openness changes the gap, contact losses, and the interpolated closed/open
material coefficients within the same running bank. Closing and reopening
does not resurrect the old open tail. There is no second resonator bank or
reverse-audio buffer.

Related physical modeling references:

- Serafin, Huang and Smith's [banded digital waveguide mesh](https://mtg.upf.edu/mosart/papers/p38.pdf)
  motivates combining resolved low modes with a dense waveguide approximation.
- [Real-Time Modal Synthesis of Crash Cymbals with Nonlinear Approximations, using a GPU](https://www.dafx.de/paper-archive/2019/DAFx2019_paper_48.pdf)
  describes the much larger nonlinear modal approach; this implementation makes
  a deliberate CPU/detail tradeoff and does not implement that solver.
- Sekiguchi and Samejima's [physical modeling and sound synthesis of a hi-hat](https://www.jstage.jst.go.jp/article/ast/44/5/44_E2293/_pdf/-char/en)
  treats the two vibrating shells and their contact explicitly. Our contact
  state is a reduced approximation of that mechanism.

## Calibration and voicing

`analyze.py` trims the detected strike onset, resamples for analysis, measures
time-dependent band powers, and estimates positive modal/region losses. It
retains source filenames and SHA-256 hashes. Filename suffixes are not assumed
to identify velocity layers, matched pairs, or the same physical cymbal.

`calibrate.py` compiles the actual synthesis basis through the pinned DGenLisp
compiler. It fits nonnegative radiation couplings against quadratic spectral
forms, retaining correlations between overlapping filters. Proposed loss
changes are accepted only after rerendering the physical network. Exponential
scales are factored out before evaluating log power to avoid overflow during
short-decay optimization.

`refine.py` adds untapered window energy and a soft crest penalty. A target crest
of 1.25 times the measured recording peak is an optimization target, not a hard
guarantee; each report records its actual ratio. The output has fixed 0.65
headroom. There is no runtime limiter or per-hit normalization. Aggressive
gain, decay and pickup settings can exceed unity and need mixer headroom.

Crash and ride interpolate all their reference-derived voices along spectral
centroid. The hi-hat deliberately exposes three stick-struck closed voices and
three open voices:

| Character | Closed reference | Open reference |
| --- | --- | --- |
| 0 | Hihat - Close_7.wav | Hihat - Open_4.wav |
| 0.5 | Hihat - Close_8.wav | Hihat - Open_3.wav |
| 1 | Hihat - Close_3.wav | Hihat - Open.wav |

These are selected voicings, not claimed matched recordings of the same pair.
Pedal/muted examples remain in the analysis but do not silently become the
default stick sound. Modal slots are frequency-matched before interpolation.
Default pickups set Bell to 0.5 for crash/hat and 0.25 for ride to favor diffuse
vibration; the full 0–2 control remains available. Pitch tracking defaults to
zero for drum use.

Broad band-power agreement alone is insufficient: narrow ringing modes can
match that objective while sounding like gamelan. Compare fine spectral
concentration and time decay, and audition the recordings beside the rendered
models. The listening examples use explicitly matched loudness; raw renders
and measurements retain the model's actual gain.

The default closed/open hat reaches a similar fine-spectrum density to its
references. Crash and ride retain fewer densely excited frequencies than their
recordings. These comparisons describe a remaining approximation, not a claim
of indistinguishable sound; `comparison.json` reports both sides explicitly.

## Reproduction

Requirements: Python with NumPy, SciPy and SoundFile; the local source folder;
the pinned compiler and toolchain installed by the repository fetch scripts.
Use a virtual environment. `OPENBLAS_NUM_THREADS=1` avoids unnecessary fitting
thread overhead. The current validated platform is Apple Silicon macOS at
44.1, 48 and 96 kHz.

```sh
./scripts/fetch_dgenlisp.sh
./scripts/fetch_dgen_toolchain.sh
python tools/pm-cymbals/analyze.py
python tools/pm-cymbals/calibrate.py
python tools/pm-cymbals/refine.py
python tools/pm-cymbals/build.py
python tools/pm-cymbals/verify.py --cpu
python tools/pm-cymbals/compare.py
```

The analysis/calibration/refinement commands accept individual family names.
Builds default to `output/staging`, including DSP, custom UI, presets and
attribution. `build.py --install` explicitly updates the factory instruments.
The builder rejects incomplete calibration or a basis-source mismatch. Finish
calibration and verification before installing a changed engine: mixing a new
exciter with old fitted couplings caused a severe audible regression during
development.

After installation, regenerate each graph sidecar from its final source and
verify compiled patch-editor roundtrip audio. Run the focused
`factory_cymbal_sidecars_preserve_executable_controls` integration test (set
`ESEQ_PM_FACTORY_DIR` to the staging factory directory to check before installing), the
three `*_surface_controls_and_pages` UI tests, production `instrument_probe`
checks, and the provided `pm-{crash,ride,hihat}-{0,1,2,3}.lisp` Metal capture
fixtures. Inspect the captured images.

The integration test exports executable graph writeback when
`ESEQ_PM_VERIFY_DIR` is set. Compile and compare that writeback against the
staged sources with `python tools/pm-cymbals/check_roundtrip.py <export-directory>`
(add `--installed` after installation). This covers two off-grid strikes and
both ends of the hat's openness range, with a maximum sample error of 2e-5.

`verify.py` checks silence, zero velocity, gate-only and off-grid strikes,
block partition invariance, every control boundary, interpolated voicings,
live automation, sample rates, and hi-hat closing/reopening. Its optional CPU
pass reuses the gamelan native benchmark loop, with warmup and seven alternating
repetitions across all six gamelan models and the three cymbals. It rejects a
cymbal median above the highest gamelan median measured in that same run.
Timings include DSP only, not compilation, allocation, Python, file I/O or UI.
See `verification.json` for source/compiler hashes, raw timings and checks.

## Compiler requirement

DGenLisp v0.1.18 fixes two bugs exposed by these instruments. Tensor grouping
previously separated filter history reads and writes into whole-block passes;
sampling a live material tensor therefore changed the exciter and waveguide
response. Single-element gathers could also be incorrectly promoted to ARM
frame-axis SIMD. The upstream fix preserves shared state in one frame loop and
recognizes the gather's internal loop. The formerly failing complete hi-hat
now matches its equivalent scalar-table version sample for sample. The factory
generator uses the corrected vector coefficient reads; it does not carry a
source-level workaround for the compiler bugs.
