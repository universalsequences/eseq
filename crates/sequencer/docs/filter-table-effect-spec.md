# Filter Table Built-in Effect

Status: experimental V1

DSP: `src/effects/filter_table_dsp.lisp`

Host integration: `src/effects/filter_table.rs`

## Semantics

Filter Table reinterprets wavetable-frame harmonic magnitudes as a filter
response. It is not a resonant low-pass:

- `frame` morphs between 64 magnitude rows.
- `cutoff` assigns table harmonic 24 to the selected frequency, translating the
  table's character along the frequency axis.
- `resonance` applies bounded power-law contrast. The resulting response uses
  capped RMS makeup (at most +6 dB), so resonance changes spectral selectivity
  rather than acting as an unbounded output-gain control.
- `mix` is an equal-power crossfade between latency-aligned STFT dry and wet
  paths.

`frame`, `cutoff`, `resonance`, and `mix` expose the standard four host
modulation lanes. `output` is intentionally not modulatable.

The panel publishes each live magnitude bank to eseqlisp's frame-bank registry
without copying it through Lisp values. The generalized GPU wavetable renderer
shows the 64 non-negative magnitude rows and highlights the row selected by
`frame`. A read-only EQ8 spectrum view overlays the pre-effect input spectrum
with the target response computed from the same frame interpolation, harmonic-24
cutoff translation, resonance contrast, and capped RMS makeup used by the DSP
before finite-IR windowing. The response display is logarithmic from 20 Hz to 20 kHz; increasing `cutoff`
therefore visibly translates the selected table curve toward higher frequencies.

The response path is:

```text
magnitude row -> stride band-limit -> cutoff resample -> contrast -> mirror
-> IFFT -> wrapped Hann IR bound -> FFT -> input STFT multiply -> IFFT/OLA
```

The wrapped IR window is required. Without it, long responses wrap around the
FFT frame as circular-convolution time aliasing.

## Runtime configuration

- FFT size: 2048
- hop: 512 (4x overlap)
- table: `[64, 1025]` mutable magnitude tensor
- bounded IR: approximately 768 taps
- transforms: accelerated host FFT (`@backend accelerated`, C-only)
- wet-path/STFT-bypass latency: one FFT window (2048 samples)

Every response control is `hop-hold`ed before tensor arithmetic so the response
rebuild remains hop-gated rather than running per sample.

### Response automation smoothing

`frame`, `cutoff`, and `resonance` pass through a per-sample scalar one-pole
(`SMOOTH-MS`, 30 ms) *before* the hop-hold. Hop-quantized host automation
therefore cannot staircase the rebuilt response: successive hop responses
differ by near-continuous parameter increments, and the 4x-overlap synthesis
crossfades between them. The smoother seeds itself to the incoming value on
the first processed sample, so static parameters have no startup glide, and
`SMOOTH-MS 0` restores the legacy instant behavior (used by the fail-before
regression). All added per-sample work is scalar; tensor execution stays
hop-gated (verified against the generated C). Trade-off: a parameter jump now
reaches its target over roughly 3x the time constant (~90 ms).

`mix` is intentionally unsmoothed here — it is sample-rate modulatable and
does not rebuild the response.

### Cutoff anti-aliasing

Closing `cutoff` below the reference frequency (`REFERENCE_HARMONIC *
samplerate / N`, about 517 Hz at 44.1 kHz) resamples the magnitude row with a
uniform stride greater than 1 bin, which silently drops (aliases) features
narrower than the stride — a one-bin passband can vanish entirely depending
on alignment. The DSP now pre-band-limits the row with an a-trous cascade of
dilated 3-tap smoothing passes (effective width doubling per level, four
levels covering strides up to the 40 Hz cutoff floor) and linearly blends the
two levels bracketing `log2(stride)` per hop. Strides <= 1 (cutoff at or
above the reference) select the untouched row, so the anti-alias path is
exactly neutral for upward translation. `AA-MAX-LEVEL 0` restores the naive
resample (used by the fail-before regression). All passes are hop-gated
tensor work whose cost is small next to the existing FFT pair.

### Audio-mode perceptual smoothing

The Audio analysis mode additionally smooths each analyzed row with a
constant-Q box average spanning one sixth of an octave
(`octave_fraction_smooth`, half-width ratio 2^(1/12)), then re-normalizes to
the row's original peak. Frame-local broadband analysis otherwise bakes noisy
bin-level spectral masks that sound like stem-separation artifacts. Bin 0
(DC) keeps a zero-width band and is untouched, so the mode's DC policy is
unchanged. The width is a conservative default chosen mechanically; listening
iteration may retune it (tracked as a tuning gap, not a design decision).
Wavetable, single-cycle, and impulse-response modes are deliberately not
smoothed — their rows are exact spectra, not statistical estimates.

## Imported audio

Dropping a supported audio sample on the panel runs offline preprocessing
under an explicit **analysis mode** (`filter_table::AnalysisMode`). All modes
share a deterministic channel downmix (average) up front and end with forward
FFT, non-negative half-spectrum magnitude extraction, and per-frame spectral
peak normalization to 0..=1. They differ in framing, windowing, DC policy,
and amplitude normalization — ordinary audio and wavetables are
mathematically different inputs:

| mode              | framing                                   | window      | DC      | amplitude norm |
|-------------------|-------------------------------------------|-------------|---------|----------------|
| `wavetable`       | whole aligned 2048-sample cycles          | rectangular | kept    | per-frame RMS  |
| `single-cycle`    | one periodic resample of the whole source | rectangular | kept    | RMS            |
| `audio`           | 64 windows evenly spaced in time          | Hann        | removed | per-frame RMS  |
| `impulse-response`| first 2048 samples (faded if truncated)   | rectangular | kept    | none           |

- **Wavetable** requires a source length that is an exact multiple of 2048;
  anything else is a deterministic error, never a silent fallback. Table
  frame f sits at fractional cycle position `f*(K-1)/63` and is the
  sample-wise linear interpolation of the two adjacent whole aligned cycles;
  analysis windows never straddle a cycle boundary, so a 256x2048 wavetable
  imports as 64 selections/interpolations of whole cycles. No window function
  is applied because each frame is exactly periodic, and DC is kept because
  reference tables carry deliberate DC energy.
- **Single cycle** treats the whole source as one period, linearly resampled
  to the 2048-sample analysis cycle and repeated across all 64 frames.
  Sources longer than one cycle are decimated by the same linear resampler
  (aliasing accepted and documented).
- **Audio** uses a leakage-controlled Hann-windowed STFT over 64 windows
  evenly spaced in time; the window mean is removed so window-induced DC bias
  cannot masquerade as filter DC gain. Each analyzed row is then perceptually
  smoothed with a 1/6-octave constant-Q average (see "Audio-mode perceptual
  smoothing" above); novelty/onset-informed framing is a documented future
  extension.
- **Impulse response** takes the source's spectrum directly — the IR *is*
  the filter — with no DC removal and no amplitude normalization. IRs longer
  than 2048 samples are truncated with a 256-sample half-Hann fade. The
  single response repeats across all frames. This mode is never
  auto-recommended (an IR is indistinguishable from short audio) and must be
  chosen explicitly.

**Mode selection.** `recommend_mode` proposes a mode from the source length
(multiple of 2048 with ≥2 cycles → wavetable; ≤2 cycles long → single cycle;
otherwise audio). The recommendation only proposes: the chosen mode is shown
in the panel next to the table name, can be switched after import (the
`set-filter-table-mode` host command re-analyzes the same sample; the panel
button cycles modes), and an explicit `mode` on `set-filter-table-source`
overrides detection entirely. The chosen mode is embedded in the persisted
table reference (`"<sample-ref>#ft-mode=<tag>"`, `encode_table_ref` /
`decode_table_ref`), so save/reload and undo/redo reproduce the identical
analysis; legacy bare references fall back to the recommendation.

The `cutoff` reference harmonic is the single named constant
`filter_table::REFERENCE_HARMONIC` (24), mirrored by the DSP source. The
asset format below stores it as per-asset metadata (default 24); the DSP
currently supports only the default, and assets declaring another value are
rejected with an actionable error.

Phase is deliberately discarded. Interpolating complex spectra would create
phase cancellation between adjacent frames and make frame morphing unstable.
Silent rows are retained as zero-magnitude rows; an entirely silent source is
rejected with a per-mode deterministic error.

A fresh instance receives an original, reproducible procedural bank. No
third-party factory tables are included.

## Asset format (`.fltab`)

`effects/filter_table_asset.rs` defines the durable asset model (eseq-dtx.6):
instead of re-analyzing an audio file on every load, an asset stores the baked
64x1025 linear magnitude bank the runtime consumes, plus versioned metadata.

Container layout, format version 1: 8-byte magic `FLTABLE\n`; u32 LE header
length; human-readable JSON header; then frames×bins little-endian f32 linear
magnitudes, row-major by frame (bit-exact round-trip). The header
(`FilterTableAssetMeta`) carries `format_version`, `name`, `frames`/`bins`,
`reference_harmonic` (per-asset, default 24), `magnitude_floor`, provenance
(`analysis_mode`, `source_name`), optional `default_controls`, and an opaque
dB-domain `recipe` reserved for the preset generator and response editor
(eseq-dtx.7/.8). Readers reject — with actionable errors — bad magic,
truncation, versions above their own, dimension mismatches, unsupported
reference harmonics, and non-finite or negative magnitudes.

An asset is referenced as `fltab:<stem>` in the persisted `table` field and
resolves by stem under `filter-tables/` in the working directory first, then
the bundled factory directory `crates/sequencer/assets/filter-tables/`.
Loading an asset (any `.fltab` path passed to `set-filter-table-source`)
skips analysis entirely; analysis modes do not apply to baked assets and the
mode command reports that. The procedural default table remains generated in
code and needs no asset file.

## Factory presets and the generator (eseq-dtx.7)

`effects/filter_table_presets.rs` defines the original factory library and a
deterministic generator; the `generate_filter_tables` bin bakes it:

```
cargo run -p sequencer --bin generate_filter_tables -- \
    [--out <dir>] [--probes <dir>] [--no-audio]
```

`--out` defaults to the bundled `assets/filter-tables/` directory. With
`--probes`, each preset also emits a PPM magnitude heatmap (frames top to
bottom, bins left to right, dB grayscale) and two audio probes rendered
through the real bundled DSP with a full 0→1 frame sweep at the identity
cutoff, one event per table frame: a 24-harmonic sawtooth stack (`-saw`) and
a 30-partial broadband spread (`-spread`). Both probe signals are sustained
on purpose — the render harness's built-in probe signal decays inside 250 ms
and cannot show frame motion — and the sweep events are snapped to the 512
render block, because the harness splits a block at an event frame and the
STFT effect stops emitting after a partial (non-hop-aligned) block. Review
is programmatic (windowed RMS + spectral-centroid trace per probe) plus
manual listening sign-off.

Authoring model: a preset is a recipe of additive dB-domain elements
(low/high-pass slopes, Gaussian peaks/notches, combs with optional
inharmonic stretch, notch banks, peak banks, scattered peak clusters, tilt,
integer-harmonic masks, seeded value noise) evaluated on an octave
coordinate relative to the reference harmonic, so the runtime `cutoff`
control transposes every preset by construction. Policies are explicit: sum
in dB, per-frame peak normalization to 0 dB in the dB domain, clamp at the
recipe's `db_floor`, convert to linear only when baking. Generation is fully
deterministic (every stochastic element is seeded, and the seeds live in the
recipe); the complete recipe is embedded in the asset's `recipe` field, and
`bundled_factory_assets_match_their_recipes` fails if the baked files drift
from the in-code definitions — regenerate with the bin after editing them.

### Trajectory vocabulary

Every element parameter is a `Traj`, not a constant: the trajectory is what
gives a preset its character, and a purely linear library morphs politely
and says nothing. `frame` is normalized 0..1.

| trajectory | shape | used for |
| --- | --- | --- |
| `linear` | straight ramp | one-way glides, depth fades |
| `wobble` | ramp + sine of `cycles` periods, `± depth`, `phase` turns, amplitude `exp(-damp·frame)` | gulps, wubs, overshoot/bounce; `from == to` with integer `cycles` is a seamless loop |
| `swoop` | exponentially eased ramp (`bend > 0` loiters then rushes, `< 0` leaps then settles) | swallowing edges, leaps that settle |
| `logistic` | normalized S-curve with `steepness`/`center` | a response that snaps open at one point in the morph |
| `steps` | `count` seeded LCG values held per segment, `glide` = fraction spent moving | stepped combs, teleporting bands |
| `segments` | piecewise-linear breakpoints | vowel paths, harmonic staircases |

`NotchBank`/`PeakBank` additionally take a `stagger`: member *i* runs the
bank trajectory at time `frame + stagger·i` wrapped into 0..1, turning a
bank sweep into a barber-pole. `ScatterPeaks` gives each peak its own LCG
stream and its own step clock — the "sprlonk" primitive: clusters that
scatter rather than travel.

### Motion classes

Wild trajectories break the original "smooth intentional frame motion"
regression, which was correct only for glides. Each recipe therefore carries
a `motion` class and is validated against class-appropriate bounds
(`factory_presets_are_valid_and_move_within_their_motion_class`). All
classes assert the same sanity floor: finite magnitudes inside
`[db_floor, 1]`, every frame peak-normalized, deterministic re-bake, and a
minimum excursion from frame 0 (max over frames, not endpoint distance, so
looping presets still count as moving).

- **glide** — the original envelope: worst adjacent-frame step < 0.25 rms, no
  dB step above 4× the mean, and at most 2 direction changes of the
  per-frame spectral centroid.
- **wobble** — continuous but non-monotonic: steps < 0.6 rms, dB steps within
  12× the mean, centroid range > 0.5 octaves, **at least 3 centroid
  reversals**, and it must *fail* the glide bounds (a wobble preset that
  passes them is just a glide preset).
- **jump** — discontinuous on purpose: steps bounded < 1.0 rms, must show
  real discontinuities (a dB step ≥ 4× the mean, or ≥ 4 centroid
  reversals), centroid range > 0.5 octaves, and it must fail the glide
  bounds.

`gulp_and_sprlonk_presets_would_fail_the_glide_bounds` pins the point of the
class system: `gulp-throat` and `sprlonk` are asserted to violate the glide
envelope while `comb-bloom` satisfies it.

### The library

Twenty-one factory presets, all original curves (no third-party preset data
was used or transformed):

*Glide* — `comb-bloom` (harmonic comb fading in), `glass-comb` (inharmonic
stretch comb), `phase-flower` (six-notch phaser bank sweep), `tilt-horizon`
(dark↔bright tilt), `odd-even` (harmonic-parity mask morph).

*Wobble* — `swoop-low` (resonant lowpass climbing four octaves on a
three-cycle wobble), `cavity-high` (highpass edge dropping fast with a
resonance overshooting below it), `band-flight` (resonant bandpass climbing
in loping arcs), `notch-drift` (notch plus the resonance above it lurching
up in 2.5 wobbles), `vowel-drift` (three formants, each on its own curve),
`talkbox-cycle` (closed five-vowel loop; frame 63 meets frame 0),
`gulp-throat` (the archetype gulp: high-Q formant pair swooping three
octaves with settling wobble), `gulp-choir` (three staggered voices, a
seamless barber-pole gulp), `wub-gate` (four wubs of a resonant edge with a
counter-phase notch), `rubber-neck` (resonance flung four octaves,
overshooting and bouncing), `dust-veil` (seeded scrolling spectral texture).

*Jump* — `sprlonk` (five needle resonances scattering eleven times on
independent clocks), `droplet` (three hair-thin peaks jumping fourteen times
through a rising window), `stutter-band` (a band teleporting between seven
positions while a comb re-spaces), `arp-harmonic` (a needle stepping the
harmonic series 1-8 over a tonic band), `comb-lurch` (comb spacing and
inharmonic stretch both re-thrown eight times).

The retired `glide-low`/`glide-high` pass-filter sweeps were replaced by
`swoop-low`/`cavity-high`: a linear slope fade reads as a slightly odd
static filter, not as motion.

The presets resolve through the normal `fltab:<stem>` lookup. There is no
dedicated browser listing for bundled filter tables yet (the same is true of
bundled IRs); loading currently goes through `set-filter-table-source` with
an asset path or reference.

## Response editor (eseq-dtx.8)

The editor is a document/command model independent of any widget
(`effects/filter_table_editor.rs`): an `EditorDoc` holds frame-indexed
frequency-response magnitudes in dB (floor −80 dB, ceiling +24 dB) plus an
ordered `EditOp` history with an undo cursor. `bake()` resamples the
document's frames (variable count, 1–256) to the runtime's 64 rows with the
same deterministic mapping structured wavetable import uses, then converts
to validated linear magnitudes — the displayed table and the runtime tensor
are therefore the same numbers by construction (asserted bit-exactly in
tests).

**Ops** (all validated, all undoable): `Draw` (pencil points on one frame),
`Parametric` (additive node on the octave axis relative to harmonic 24 —
peak/notch/lowpass/highpass/tilt; transposes with `cutoff` like factory
presets), frame `Insert`/`Duplicate`/`Delete`/`Move`,
`InterpolateFrames` (keyframe interpolation between anchors),
`SmoothSpectral`/`SmoothTemporal`, `ShiftOctaves`, `StretchOctaves`,
`Tilt`, `Normalize`. Drag gestures coalesce via `replace_last` — one
gesture, one undo entry.

**Persistence.** A saved edit is a user `.fltab` asset: the payload is the
baked table (plays everywhere), and the asset `recipe` carries the full
nondestructive document (base dB grid base64-encoded + applied ops), so
reopening the editor on a saved asset restores its history. Saves go
through the recorded-mutation path — app-level undo returns the device to
the pre-save table. Closing a dirty session rolls the device back to the
table it had when the editor opened.

**Session + UI.** One session at a time (`filter-table-editor-*` host
commands: open/close/op/band/add-node/undo/redo/frame/save), bound to a
track or bus device (racks excluded, matching the analysis-mode
limitation). While open, the device panel swaps its spectrum overlay and
knob row for the editor: toolbar (frame stepper, undo/redo/save/close), a
`response-curve-editor` surface whose draggable band edits the newest
parametric node (band drags preview live without entering history until
commit; a pinned disabled point marks harmonic 24 = the cutoff reference),
and node/table/frame op toolbars. The magnitude viewer doubles as the
table overview with its highlight tracking the editor's selected frame.
Every edit auditions immediately: the baked table is written to the node
tensor and published bank without touching the prepared-table registries.

**Editor deferred**: no pencil gesture in the UI yet (`Draw` is
command/headless only — needs a drawing widget); the band curve drawn by
`response-curve-editor` is the widget's own bell/pass approximation of the
node, not the composite table response (the viewer above is authoritative);
multi-node band editing (only the newest parametric op is draggable);
save-as naming UI (saves derive a sanitized stem from the table name).

## Host behavior

Filter Table is registered alongside Convolution Reverb as a host-integrated
DGenLisp builtin. The host records the mutable tensor offset from the compiled
manifest and swaps a complete prepared bank at an audio block boundary.

Table references are persisted in projects and prepared data participates in
undo/redo mementos, allowing exact in-session replay without decoding the file
again. Project reload resolves user sources from the sample library by stem;
the procedural default requires no external asset.

## Known V1 limits

- Latency is host-compensated: `EffectDescriptor::latency_samples()` reports
  N (2048) for Filter Table, and the graph latency planner
  (`app/graph/latency.rs`) pads parallel join points (track outputs, sends,
  bus outputs) with `effects/pdc_delay` nodes so branches sum in phase.
  Dry/wet alignment *inside* the effect stays internal (both paths carry the
  same one-window latency), so the effect reports its latency outward exactly
  once — the host never double-compensates the built-in mix control. Known
  gaps: rack-slot chain joins at the voice sum are not yet padded, and an
  effect whose internal bypass drops its delay line will shift when toggled
  (latency is reported regardless of the enabled param).
- Response controls still update once per 512-sample hop, but they are
  smoothed with a 30 ms pre-hop one-pole and the resample is stride
  band-limited (see "Response automation smoothing" and "Cutoff
  anti-aliasing" above), so hop-rate automation no longer staircases and
  narrow features survive downward cutoff motion.
- The EQ8-overlay target-response display samples the raw magnitude row
  without the stride band-limit, so for cutoff below the reference frequency
  the displayed level of very narrow features can differ from the rendered
  (band-limited) response. Visual-only divergence; align it if the editor
  work (eseq-dtx.8) makes the overlay authoritative.
- Audio-mode 1/6-octave perceptual smoothing width is a mechanically chosen
  conservative default; it has not been listening-tuned.
- Minimum-phase/causal FIR mode is not implemented. A minimum-phase spectrum
  alone would still retain STFT latency; true zero-latency operation needs a
  separately profiled dynamic time-domain FIR path.
- Runtime processing uses the accelerated host FFT and therefore cannot compile
  for Metal. Removing `@backend accelerated` selects composed tensor FFTs for
  backend-portable experimentation.
