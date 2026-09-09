> Architecture revision, 2026-09-08: the playable instrument now targets **four
> voiced operators and four independent noise bands**, with six forward PM and
> sixteen one-sample feedback routes. This replaces the eight-operator target
> below following the CPU measurements. Motion schema v2 uses four entries and
> 29 tensor channels; the UI and 32 presets target the four-operator graph.
> See `tools/fm-formant/README.md` for current implementation and measurements.

# Factory FM/formant synth specification

Revision 1 · 2026-09-08 · specification `eseq-pwm4` · DSP validation `eseq-kb9y`

Implementation attempt: [playable candidate and measurements](../tools/fm-formant/README.md),
epic `eseq-9elc`. The finite-series alternative compiles and renders, but the
complete voice fails the CPU gate and the host program/motion integration remains
unfinished. This does not lower the release requirements below.

## 1. Product direction

Build an original factory instrument combining eight voiced FM/formant operators
with eight independently shaped noise operators. Its defining capability is
articulation: a note's pitch, resonant identity, breath, and movement can be
controlled independently, then connected through FM.

The FS1R is the architectural inspiration. This is not a hardware emulation,
SysEx-compatible replacement, or reproduction of Yamaha's preset bank. The
instrument must stand on its dry sound: moving vowels, synthetic choirs, breathy
reeds, metallic resonances, glass, animated bass, and unfamiliar vocal textures.

The chosen implementation direction is **compact carrier/window formant synthesis
at the host sample rate**. Explicit additive synthesis and high-rate rendering
are reference tools, not the default production engine. The exact window kernel
and its anti-aliasing treatment remain subject to the mandatory DSP gate below.

This document supersedes the implementation recommendations in the earlier
[investigation](fs1r-factory-synth-investigation.md); its measurements remain
historical evidence. Requirements below are the proposed release contract, not
claims that the implementation or performance targets have already been achieved.
The product name is not yet selected.

## 2. First-release scope

| Area | Required behavior |
|---|---|
| Voiced bank | Eight operators, each selectable as sine PM or formant PM |
| Unvoiced bank | Eight independent noise bands with their own levels and envelopes |
| FM | Editable routing among voiced operators; explicit delayed feedback |
| Formants | Independent repetition pitch, center, bandwidth, and skirt controls |
| Articulation | Per-operator amplitude and frequency envelopes; velocity and pressure response |
| Motion | Editable native keyframes, manual scanning, note-relative playback, one-bar transport mode |
| Performance | Eight assignable macros, mod wheel, pressure, pitch bend, polyphony and mono legato |
| Authoring | Readable DGenLisp composition, reusable macros, authored patcher layout, custom panel |
| Factory content | At least 32 original presets, including static and animated sounds |

Out of scope: matching all 88 Yamaha algorithms or all Yamaha waveform families,
speech analysis, audio-to-formant conversion, FS1R preset/Fseq import, a vocoder
input, internal unison stacks, full MPE, and built-in reverb/delay. Existing host
effects can accompany presets without becoming part of the synthesis contract.

## 3. Voice architecture

Each voice owns oscillator phases, envelopes, feedback history, noise states,
smoothers, and a motion cursor. Immutable program and motion data can be shared.

```text
host note / expression / clock
          |
    motion + envelopes + macro mappings
          |
    8 voiced operators <--- voiced PM graph
          |
    8 unvoiced bands
          |
    per-operator output gain + pan
          |
    stereo sum -> DC removal -> master amplitude -> output
```

A voiced/unvoiced pair is an editing convenience. Pair members may have unrelated
levels, frequencies, envelopes, and motion. Linking controls is an explicit edit
operation; it must not silently collapse two independent DSP states.

The engine supports the host's existing ceiling of 12 voices. Voice allocation,
stealing, mono priority, glide, and expression delivery use shared host behavior.
Do not add a private note allocator or transport inside the instrument. Missing
shared behavior must be implemented at its owning layer.

### 3.1 Sine operator

For operator `i`, advance an unmodulated phase accumulator from its frequency and
evaluate `sin(phase_i + pm_i)`. PM is in radians. Frequency can follow note pitch
with a ratio and cents offset, or use a fixed frequency in Hz. Pitch bend affects
note-following frequency; fixed frequency stays fixed unless explicitly mapped.

Initial editing ranges are ratio 0.125–32, detune ±100 cents, and fixed frequency
10–20,000 Hz. Effective oscillator frequencies are bounded below Nyquist using
the actual sample rate. Range limits are part of the parameter contract, not a
dynamic alias detector.

### 3.2 Formant operator

The primitive generates a periodic sequence of shaped carrier windows:

| Quantity | Meaning |
|---|---|
| Repetition frequency `f0` | Harmonic spacing / perceived note pitch; uses the operator's ratio or fixed mode |
| Center `Fc` | Location of the formant's spectral envelope in Hz |
| Bandwidth `B` | Full width at −6 dB amplitude of the underlying continuous spectral envelope |
| Skirt `S` | Tail shape outside that bandwidth, independently calibrated |
| PM input | Phase displacement of the internal carrier, in radians |

PM changes the carrier inside the window; it does **not** displace the repetition
clock. Center modulation and repetition-frequency modulation remain separate
controls. This is a deliberate original synthesis law, not a claim about FS1R
internals. Whole-waveform phase warping is not an interchangeable optimization.

The primitive must handle fractional `Fc/f0` continuously. Harmonic interpolation,
bounded overlapping windows, or an equivalent compact construction may be used
only after proving their behavior at harmonic crossings. Wrapping/resetting a
carrier while the window is nonzero is not acceptable unless the resulting
waveform and derivative continuity are explicitly accounted for.

Initial user ranges: center 80–16,000 Hz, bandwidth 20–8,000 Hz, skirt 0–1.
The DSP gate must publish the supported joint domain and the mapping from these
units to kernel coefficients. Skirt 0 means broader tails and 1 steeper tails;
holding bandwidth constant must preserve the calibrated −6 dB width. If one
candidate cannot provide that independence, reject it or revise the spec before
authoring the instrument. Do not rename an uncalibrated Gaussian coefficient
“bandwidth.”

Center is fixed in Hz by default. A separate tracking control from 0–100% moves
it with note pitch relative to MIDI note 60; formant shift adds an octave offset.
Tracking and shift multiply `Fc`, while bandwidth scaling has its own control.

At high repetition frequencies, harmonics sparsely sample the envelope. The
strongest spectral line may differ from `Fc`, and some vowels lose their identity.
The UI displays an envelope, not a promise of a partial exactly at the center.

### 3.3 Unvoiced operator

Each band shapes an independent, zero-mean noise stream with center and bandwidth
in Hz, an amplitude envelope, center-frequency envelope, and output pan. Use a
stable, calibrated bandpass implementation; characterize bandwidth and gain
across sample rates. Do not share one noise stream across all eight bands.

Noise states are independent across voices and bands. Tests use explicit seeds;
production seeding must be repeatable under deterministic offline rendering and
must not depend on wall-clock timing or thread scheduling.

Noise is mixed into the output in release one; it is not an FM graph source.
Bandwidth never reaches zero. No automatic switch from narrow noise to a sine is
allowed. Bandwidth-dependent gain compensation is deterministic and documented;
there is no automatic loudness normalization that pumps during articulation.

### 3.4 Envelope and note lifecycle

Every voiced and unvoiced operator has an amplitude ADSR and a bipolar frequency
ADSR. Frequency-envelope depth is in octaves and applies to sine frequency or
formant/noise center respectively. A voiced formant's repetition pitch also
receives the shared note-pitch envelope. Master amplitude has a separate ADSR.

Envelope segments specify time in seconds and a documented curve. Release starts
from the current value. Zero-length stages advance deterministically within the
current sample; no division by zero or block-sized minimum is permitted.

Normal note-on retriggers envelopes and motion. Phase reset is a program setting:
reset or free-running on reused voices, with reproducible initialization. Mono
legato preserves phases and envelopes unless envelope retrigger is enabled.
Feedback histories initialize to zero on a newly assigned voice. Reassignment
must follow the host's click-controlled stealing policy.

## 4. FM routing and controls

Separate three quantities: operator amplitude, PM edge depth, and audible output
gain. Muting an operator's audible output must not remove its modulation. Muting
its operator amplitude removes both its modulation contribution and its sound.
Pan is applied only to the audible output, not to the scalar PM signal.

Let `v_i[n]` be the operator waveform multiplied by its amplitude envelope and
operator level. PM input is:

```text
pm_i[n] = sum(feedforward_depth[j,i] * v_j[n])
        + sum(feedback_depth[j,i] * v_j[n-1])
```

Depth is radians per unit source amplitude, initially −16 to +16 per edge.
The signed range permits cancellation and phase inversions without requiring
duplicate operators. Feedforward edges form a directed acyclic graph and execute
in topological order. Every self-loop or cyclic connection must be explicitly
marked delayed; the UI shows its one-sample delay. Never insert implicit delays
on all edges to simplify evaluation. Feedback delay is one host sample, so its
physical duration varies with sample rate; identical feedback timbre across
sample rates is not promised.

Provide at least eight original routing templates: parallel carriers, independent
pairs, long stack, branching stack, common modulator, summed modulators, formant
carriers, and feedback texture. Templates instantiate the same editable graph.
There is no special hidden DSP path per preset.

Graph topology and operator mode are program edits, not sample-accurate
modulation destinations. Validate and prepare changes off the audio thread.
Active voices finish on their immutable program revision; new notes use the new
revision. Hold at most two live program revisions. Further structural edits
replace a pending revision and are published after the old voices retire; the UI
shows pending state. Never accumulate unbounded engines during editing.

Continuous edge depths, levels, centers, widths, and macro values are automatable.
UI changes and parameter locks enter the same DSP smoothing path. Initial
smoothing is a 2 ms one-pole time constant for continuous base values: log2 Hz
for positive frequencies, linear amplitude for gain, radians for PM depth.
Audio-rate PM and authored envelopes bypass that base-value smoother. Abrupt
motion scanning uses the same target smoothing; continuous timeline interpolation
does not add a second smoothing stage. Exact time constants become saved DSP
semantics and must not vary with block size.

## 5. Native-rate sound quality

There is no mandatory whole-engine oversampling path in release one. The plan is
to control spectral generation at the operator, characterize its useful domain,
and improve measured failure cases without silently reducing the user's FM depth.

Required investigation order:

1. Choose continuous windows with controllable spectral tails and bounded state.
2. Characterize center, bandwidth, skirt, and harmonic-crossing behavior without PM.
3. Add PM, then stacks and delayed feedback; measure each new source of aliasing.
4. Evaluate native-rate corrections where appropriate, including analytical
   band limiting, calibrated table families, or antiderivative methods.
5. Profile the complete implementation before replacing math with approximations.

Band-limited tables alone do not make arbitrary PM band-limited. A low-pass filter
after the voice cannot remove components that already folded into the audible
band. Frequency/index heuristics can guide preset design, but are not proof of
alias rejection for arbitrary graphs. Feedback filtering may be an explicit
sound-shaping control; it must not masquerade as a universal anti-alias solution.

Do not silently lower PM depth with pitch, clamp formant width according to an
undisclosed heuristic, or attenuate high notes to make a benchmark pass. Necessary
sample-rate domain bounds must be documented and visible. Any additional
protective voicing mode would require an explicit, saved parameter and measured
tradeoffs before being added to scope.

The existing Gaussian PAF probe is not production DSP: it lacks independent skirt
calibration and showed severe high-frequency and strong-PM folding. Its final
eight-pair microbenchmark was about 44 µs per 128 samples on one M1 Max run, with
an earlier much noisier result. Neither value proves complete-voice CPU cost.
See the [measurement report](fs1r-factory-synth-investigation.md) and
[reproduction harness](../tools/fs1r-research/README.md).

## 6. Motion and morphing

Motion is a native, versioned program asset. A shared timeline contains 2–64
ordered keyframes at normalized positions from 0 to 1. Each frame stores, for
each of eight pairs, voiced/unvoiced level and center, voiced/unvoiced bandwidth,
and voiced skirt, plus a shared pitch offset in semitones. Store absolute values
in physical units; absent legacy fields are not relevant to the initial format.

Motion amount blends the base program toward the sampled frame. Levels interpolate
in linear amplitude, frequencies and widths in log2 space, skirt linearly, and
pitch offsets in semitones. All lanes share the same time coordinate, preserving
the relationship between body, noise, and pitch. Frequency modulation and global
formant shift are applied after this blend. Operator envelopes then multiply
levels and offset frequencies according to their documented units.

Playback modes:

| Mode | Timing and restart |
|---|---|
| Manual | Position parameter scans the timeline; fully automatable |
| Note one-shot | Starts at note-on; duration in seconds or bars; holds final frame |
| Note loop | Starts at note-on; loops while the voice exists |
| Transport one-bar | Reads the host's modulo-one-bar phase directly |

Looped motion requires matching first/last frames in every lane. The editor has
an explicit close-loop operation; invalid loop assets fail validation. Note-off
releases amplitude while motion continues, avoiding a sudden change in the
release sound. Mono legato restarts motion only when its retrigger setting is on.
In bar-duration modes, progression follows the delivered host clock increment;
zero increment freezes motion. Second-duration modes continue independently of
transport. Transport mode follows seeks and resets through the supplied phase.

Multi-bar absolute song synchronization is excluded until the host exposes a
proper absolute transport position/discontinuity contract. Counting wraps of a
one-bar phase cannot establish the correct position after arbitrary seeks.

### 6.1 Persistence and publication

Current scalar preset maps do not provide a complete motion-asset contract.
Implement versioned structured program data in the shared instrument persistence
layer, covering graph topology, macro mappings, and motion. Use immutable,
content-addressed assets referenced by programs; preserve them through preset
save/load, project save/load, duplication, factory fork, export, and undo/redo.
Do not encode structured data into undocumented float parameter slots.

Parsing, validation, storage, lookup-table preparation, and compilation happen
off the audio thread. Publish one complete revision at a block boundary, shared
by all subsequently allocated voices. Existing voices keep their revision.
Reclaim old assets and compiled programs off the audio thread. The existing
tensor-write API alone does not establish these ownership guarantees.

Validation rejects nonfinite values, unsupported versions, missing references,
out-of-range fields, duplicate/unsorted times, invalid graph cycles, and capacity
overflow. A failed edit leaves the last valid program sounding and exposes a
specific error. Runtime memory and work are bounded by validated capacities.

## 7. Performance interface and authoring

The primary panel presents eight macros, output, articulation, and motion controls.
Initial macro roles are vowel position, formant shift, width, FM intensity,
breath, brightness/skirt, attack character, and motion depth. Each preset gives
them useful names and ranges; mappings are program data, not hard-coded special
cases. Macros change continuous destinations only. Velocity defaults to master
amplitude, with optional operator-specific timbre depth. Pressure and mod wheel
use explicit macro mappings. Do not advertise expression dimensions the host
does not deliver.

An operator page shows all eight pairs, with one selected pair's detailed controls,
envelopes, and a spectral-envelope preview. A routing page shows audible carriers,
PM depths, and delayed edges. A motion page edits keyframes and previews the same
interpolation law used in DSP. Pair copy/paste, mute, solo, and link edits must be
undoable. Solo affects monitoring without destructively rewriting the program.

Canonical parameter IDs are independent of labels and layout, for example
`voiced_1_center_hz`, `unvoiced_1_level`, `pm_1_to_2_depth`, `motion_position`, and
`macro_1`. Enum IDs are stable named schema values, not incidental UI indices.
Program edits and continuous host parameter changes have distinct ownership.

Compose the factory source from meaningful operator, envelope, routing, motion,
and output macros with authored recursive layouts, following the
[factory macro specification](factory-macro-library-spec.md). Keep host-modulatable
parameter declarations at top level and pass their modulation signals into
macros; the currently audited compiler does not preserve macro-local modulation
destinations. Verify the actual shipped manifest rather than assuming support.

If the robust kernel needs compiler/runtime support, add a reusable primitive
with a small documented contract. Do not hide the complete synth in an opaque
native callback, duplicate the host, or depend on generated-C patches. Factory
assets must load from the installed content system without development paths.

## 8. Acceptance gates

### Gate A — operator contract, before full instrument authoring

Freeze equations, phase/reset law, normalization, coefficient ranges, overlap
capacity, bandwidth/skirt calibration, interpolation, and state layout. Include
an independent float64 reference. A kernel that cannot meet the joint control
contract must be rejected or trigger an explicit spec revision.

Static clean-domain tests cover `f0` 55–440 Hz, center 200–5,000 Hz, bandwidth
80–1,200 Hz, and skirt at 0, 0.5, and 1, including fractional centers and harmonic
crossings at 44.1, 48, and 96 kHz. Initial acceptance target is relative RMS error
at or below −60 dB against a converged, ideally low-passed high-rate reference,
with matched gain and phase and no noise bands. This is a **proposed quality gate**,
not a measured result. Report total numerical error separately from identified
alias energy; a single residual number is not a listening verdict.

Reference rendering must converge when its rate is increased, and its floor must
be at least 20 dB below the acceptance threshold. Include explicit harmonics
where available. For feedback, preserve the production delay's physical duration
in the reference; changing one-sample delay to one high-rate sample changes the
instrument and invalidates direct residual comparisons.

PM tests include two-operator cases, long stacks, summed modulation, feedback,
high notes, center sweeps, extreme width/skirt, and fast envelopes. No universal
−60 dB promise applies to every extreme feedback patch. Publish the alias and
spectral-error maps and listen to exposed dry examples before approving the
musically useful domain. A failed clean-domain gate cannot be waived by calling
all factory content experimental.

### Gate B — complete voice and host integration

Render the complete eight-pair workload, envelopes, motion, routing, feedback,
and stereo output through `instrument_probe`. Cover silence, note-on/off,
retrigger, legato, stealing, automation, revision swaps, and release retirement.
Validate no nonfinite output, runaway state, unexplained gain jumps, or allocation,
locking, compilation, and asset destruction in the new audio path.

Compare renders with 32/64/128/256-sample blocks, preserving event times. Require
deterministic seeds and a declared floating-point tolerance; target peak difference
below 1e-5 for normalized non-chaotic cases. Feedback cases need an explicitly
justified tolerance or spectral/state invariant, not an unexplained failing test.

Benchmark the actual published compiler/toolchain on Apple Silicon macOS and
x86_64 Linux. Record revisions, CPU, rate, block size, warm-up, median, p95, and
maximum block cost. The initial M1 Max target for 12 fully active voices at
48 kHz/128 frames is median compute cost below 25% and p95 below 35% of one core's
block time, measured without contention and including instrument dispatch.
This is a budget, not the earlier microbenchmark multiplied by 12. Also measure
96 kHz and the two-revision edit case. Record Linux results against a named
machine before setting its release budget; portability alone is insufficient.

If the budget fails, profile active branches, recurrences, buffer traffic, and
compiler scheduling. Do not reduce operator count or freeze inaudible state
without an explicit documented design change and behavioral tests.

### Gate C — program and UI integrity

Round-trip structured programs and assets through preset/project save, reload,
fork, duplicate, export, and undo/redo. Check immutable revision behavior with
sustaining notes and repeated edits. Test missing/corrupt assets and ensure the
prior valid program survives a rejected publication.

Run narrow functional UI/layout tests for binding and finite nonzero geometry,
including reactive values. Use a real `metal_seq capture` fixture and inspect the
PNG for clipping and usability. Do not assert decorative copy or exact styling.
Verify live parameter locks, recording, and playback follow shared host semantics.

### Gate D — factory sound and packaging

Ship at least four presets in each of eight families: vowels/choirs, breath/reeds,
glass/bells, metallic percussion, bass, keys, evolving pads, and abstract motion.
Every preset has deliberate velocity response and useful macro mappings. At least
half demonstrate motion; several must establish that static formants also work.

Audition dry at low, middle, and high pitches and across macro extremes; assess
unwanted whistles, grit, clicks, dead ranges, and loudness jumps. Automated checks
cover bounded output and release; human approval establishes musical quality.
Effects must not conceal a defective core. Test installed assets on clean macOS
and Linux environments using the pinned published tools. No user-directory
compiler, research scripts, or external speech assets may be required.

## 9. Delivery dependencies and unresolved evidence

The next implementation gate is tracked by `eseq-kb9y`. Delivery proceeds from
kernel validation to complete voice/routing, structured program persistence and
motion, then factory panel/presets and installed-platform validation. These are
dependency boundaries; task status belongs in Beads.

The unresolved questions are the precise window construction, the supported
bandwidth/skirt domain under PM, native-rate correction cost, and complete voice
performance on both platforms. The present probe does not settle any of them.
Completion of this spec does not establish that a production engine exists.

## 10. Source boundaries

The [Yamaha owner manual](https://data.yamaha.com/files/download/other_assets/5/333335/FS1RE1.PDF)
documents voiced/unvoiced operators and warns that some non-sine settings can
produce aliasing. Its architecture is inspiration, not evidence of alias-free
behavior. The [data list](https://usa.yamaha.com/files/download/other_assets/4/317954/FS1RE2.PDF)
describes Yamaha's parameters and Fseq representation; this spec uses an original
native representation.

Yamaha's [carrier/window patent](https://patents.google.com/patent/JP2504173B2/en)
describes economical formant generation, while a
[related patent](https://patents.google.com/patent/JP3405170B2/en) describes
time-multiplexed channels. Neither proves the FS1R's exact shipping kernel or
internal sample rate. High chip-clock throughput is not itself audio oversampling.

[Puckette's PAF discussion](https://msp.ucsd.edu/techniques/v0.07/book-html/node88.html)
and the [Csound FOF documentation](https://csound.com/docs/manual/fof.html) supply
useful alternative constructions and tradeoffs. Hardware-era lookup/log-domain
techniques explain feasibility but do not establish the fastest modern compiled
implementation. Optimize only against the actual shipped toolchain.
