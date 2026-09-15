# PM Cello

One physical string/body model produces the sustained crescendo, pizzicato and
spiccato references in `samples-to-analyze`. The presets change ordinary exposed
parameters. The instrument contains no recordings, resampled attacks, wavetables,
or per-reference synthesis branches.

## Playing it

Load **Physical Models / PM Cello**. The three `Reference ...` presets correspond
to the supplied files. Their reference pitches are approximately 110.32 Hz,
97.66 Hz and 389.62 Hz respectively. The two section filenames contain conflicting
pitch labels; these frequencies were measured from the actual audio.

**Held Bow** provides a playable sustained version with gentle delayed vibrato.
**Bowed Wire** uses the same model with dispersion, sharper contact and a smaller,
less dominant body to demonstrate a metallic direction.

The bow's **Stroke limit ms** automatically lifts the bow for a repeatable gesture.
Set it to **0** to follow a held note. **Bow contact** blends the bow's coupling;
zero lifts it completely. **Pluck strength** fires a force pulse on a trigger or
gate rise. These excitations can operate together. The envelope controls bow
speed, while **String / Decay s** controls the free string's ringing time after
either excitation ends. A pizzicato therefore rings after note-off.

**Section** blends the central player with two instances of the same string model.
Their pitch spread, entry spacing and stereo width approximate the player spread
in the section recordings. At Section 0 the output is one mono player. Width 0
also makes a section mono. This is a fixed three-player model, not a sample chorus.

The eight pink pages expose all 39 parameters. Contact position, bow pressure,
rosin curvature, damping, stiffness and the four independently tuned body modes
cover substantially different timbres. Stiffness, extreme contact and bow settings
can produce inharmonic or unstable *musical* behavior; finite numerical state is
checked separately. Body size shifts all four modes together.

## Model

The two delay segments carry traveling velocity waves between the contact point,
bridge and finger termination. Both endpoints reflect with opposite sign. The
neck return includes a lowpass loss filter and an allpass dispersion filter. Delay
lengths account for their fundamental phase and the explicit one-sample histories.
The requested fundamental T60 is compensated for the lowpass magnitude only while
the termination remains passive; heavy damping can shorten the requested decay.

The bow injects relative velocity times a bounded memoryless friction reflection
curve. The bow-contact envelope starts promptly while the speed envelope can make
a slow crescendo. The pluck is a short half-sine force pulse, optionally mixed
with filtered noise, applied at the same scattering junction. The free string is
therefore identical after the excitation leaves. The bow model follows the
published [bow/string scattering junction](https://dsprelated.com/freebooks/pasp/Bow_String_Scattering_Junction.html)
formulation and its bounded bow-table approximation; it does not solve a full
hysteretic friction law.

Four normalized resonant bandpasses approximate bridge/body radiation outside the
feedback path. Their complex responses combine before a final tone lowpass.
This follows the separation of string excitation and body response described in
[Body Modeling](https://dsprelated.com/freebooks/pasp/Body_Modeling.html).
It does not model a vibrating bridge, coupled strings, detailed cello geometry,
bow hair, room acoustics, or individual performers' irregular motion. The panel
curves are labeled nominal analytical previews, not measurements of live audio.

These are timbre and articulation fits, not waveform-identical copies. In
particular, three deterministic players cannot reproduce the recorded section's
individual timing, beating and room tail. The bowed crescendo remains the harder
spectral fit. `validation.json` records the actual error rather than claiming a
perfect match.

## Source structure

`dsp.lisp` is organized so the patch editor shows one box per physical block.
The top level is only the signal path: `cello-age` and a shared filtered noise
source feed `cello-bow` and `cello-pluck`; `cello-pitch` applies vibrato and
tuning; `cello-strings` runs the three players (`cello-string` x3, with their
smoothed controls in `cello-string-controls`); `cello-stereo` blends and pans
them; `cello-resonance` applies `cello-body` per channel plus the tone filter;
`cello-level` scales by velocity and gain. Each block reads the parameters it
owns by name inside its macro. Modulatable parameters must stay top-level
(`(mod x)` only resolves against a bare top-level `param`), and the
parameter order is a saved-project contract (projects address parameters by
position), so the `param` block is unchanged and new parameters go at its end.

## Compiler prerequisite

This model exposed a compiler history-ordering bug: a ready flag could be written
before its previous-sample read inside a feedback region. That made a seeded
contact position start near zero and corrupted the initial traveling-wave path.
The fix is in DGen's `FeedbackAnalysis.swift`, tracked by `dgen-3vz`, with a regression
that fails on the original compiler at block sizes 1, 8 and 128. The fix adds
scheduling dependencies from same-cell reads to writes without changing graph
value or gradient dependencies. Twenty-two related compiler tests passed.

The fix is published in [DGenLisp v0.1.15](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.15),
commit `57ab23aa616a594f26ec1497dced21a338048846`, and pinned for macOS ARM64 in
`content/dgenlisp.lock`. Install it through the normal verified vendor path:

```sh
./scripts/fetch_dgenlisp.sh
```

No local compiler or audit-script override is needed for the vendored macOS
distribution. The older Linux compiler pin has not been validated for this
instrument; a successful compile alone does not establish correct history ordering.

## Reproduce the comparisons

Use Python with NumPy, SciPy and SoundFile. Keep the supplied reference files in
`samples-to-analyze`; their hashes are in `reference-analysis.json`. Analysis and
fitting read those files only offline, never at instrument load or playback.

```sh
PYTHONDONTWRITEBYTECODE=1 python tools/pm-cello/analyze.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-cello/fit.py crescendo --iterations 35
PYTHONDONTWRITEBYTECODE=1 python tools/pm-cello/fit.py pizzicato --iterations 35
PYTHONDONTWRITEBYTECODE=1 python tools/pm-cello/fit.py spiccato --iterations 35
```

`fit.py` searches physical controls with bounded differential evolution, optional
bounded local refinement and deterministic seeds. The objective combines weighted
24-harmonic trajectories in three windows with a 20 ms RMS envelope. It estimates
output gain separately and never optimizes phase. It writes candidates under
ignored `output/`; it does not silently overwrite the factory bank. The reviewed
bank is the authoritative set of voicings.

RMS envelopes use the unfiltered mono average. Pitch and harmonic measurements
remove recording rumble with a 30 Hz highpass. The level comparison uses the full
recording duration; spectral error uses three articulation-specific windows.

The final local run passed **439 audio/state renders** across seven compiled
variants. Natural bowed/plucked tuning stayed within 6.42 cents over the tested
registers and sample rates. Block-size changes were sample-identical; the largest
patch-save sample difference was 0.0000154. Peak output over the tested settings
was 0.861. These results cover the stated tests, not every possible automation.

| Reference | RMS-envelope error / reference peak | Weighted harmonic RMS error |
| --- | ---: | ---: |
| Crescendo | 6.61% | 5.96 dB |
| Pizzicato | 5.74% | 3.72 dB |
| Spiccato | 5.32% | 2.67 dB |

These errors are fitting diagnostics, not perceptual similarity percentages.
All three presets also passed the app's `instrument_probe` compile/load/init path;
see `host-probes.json`. All four physical-model panels passed their geometry,
reactive-control and parameter-lock callback tests after the shared UI extension.
All eight cello pages were inspected in production Metal captures.

Validate a fresh patch-editor save and the actual compiled sound:

```sh
ESEQ_PM_VERIFY_DIR="$PWD/tools/pm-cello/output/roundtrip" \
  cargo nextest run -p eseqlisp --test pm_woodwinds \
  -E 'test(=factory_cello_sidecar_preserves_executable_controls)'
PYTHONDONTWRITEBYTECODE=1 python tools/pm-cello/verify.py \
  --roundtrip tools/pm-cello/output/roundtrip
cargo nextest run -p sequencer --bin metal_seq \
  -E 'test(=state_values::tests::pm_woodwind_ui_tests::cello_surface_controls_and_pages)'
```

The audio harness checks complete preset recall, reference trajectories, registers,
44.1/48/96 kHz tuning, control extremes and combinations, live automation, host
modulation, silence before a note, gate-only notes, retriggers, stereo behavior,
block partitioning and patch-save audio equivalence. It audits generated feedback
code and tests finite internal state, not only the output. A/B WAVs contain the
reference first, half a second of silence, then synthesis at the preset's level.

The `pm-cello*.lisp` capture fixtures exercise the production instrument panel.
For example, on macOS:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/pm-cello.lisp \
  --buffer fx --track 0 --width 1800 --height 600 --out /tmp/pm-cello.png
```
