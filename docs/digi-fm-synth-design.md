# Digi FM: factory synth design

2026-09-08 · Design: eseq-lb52 · Implementation: eseq-md0a.

Combine the original Digitone's FM tone architecture with Digi Drift's nonlinear filter path and Operator's compact interface organization. The central colored editor follows Heat and Digi Drift. General motion comes from the host modulation system. This proposal does not replace the separate FM Formant experiment.

## Reference architecture

Reference: original Digitone OS 1.41, especially synth parameters and Appendix A; not Digitone II's additional machines. Four operators are named C, A, B1, B2. Preserve those names in the UI and DSP so the routing diagrams remain unambiguous. The eight routing definitions below are transcribed from the diagram on printed page 90. `d` means direct audio output; `e` means envelope-controlled audio output. These describe connectivity, not a claim of measured DSP equivalence. [Digitone manual](https://www.elektron.se/wp-content/uploads/2024/09/Digitone_User_Manual_ENG_OS1.41_231108.pdf#page=90)

| Algorithm | Modulation edges | Feedback | X | Y |
|---|---|---|---|---|
| 1 | B2→B1; B1→C; A→C | A | C d | B1 d |
| 2 | A→C; B2→B1 | B2 | C d | B1 d |
| 3 | A→C,B1,B2 | A | C d + B2 d | B1 d |
| 4 | B2→B1→A→C | B2 | C d | B1 d |
| 5 | B2→B1,A; B1→A; A→C | B1 | C d | A d |
| 6 | A→C,B1; B2→C,B1 | A | C d | B1 d |
| 7 | A→C; B2→B1 | A | C d + A e | B1 e + B2 e |
| 8 | A→C | B1 | C d + B2 e | B1 e |

Store the algorithm definitions once, including output taps and feedback ownership; derive DSP routing and UI diagrams from that definition. Preserve all branches even where output carriers and modulators overlap. A selector with eight names is insufficient evidence of eight correct algorithms.

## What the existing experiments establish

`.local/instruments/emulations/digitone/dsp.lisp` already contains four phasors, although B1/B2 have identical tuning expressions. Its algorithm 1 routes B2 into A instead of routing B1 into C; algorithm 4 duplicates C on X and Y; its feedback ownership is wrong for several algorithms. It delays every modulation edge, applies envelopes to several nominally direct outputs, uses a 30-second ADSR release as a hold approximation, and implements harmonics as self-PM. Preserve the musical intent, not these implementations.

`.local/instruments/core/operator/dsp.lisp` offers a useful same-sample, feedforward routing example. Its algorithms and per-operator ADSRs describe a different architecture. Its UI uses two operator columns, a narrow detail column, and additional LFO/pitch strips. Reusing that layout would retain the width problem.

The inspected working-tree Digi Drift has Type I nonlinear SVF and Type II Sallen-Key cascade macros, followed by its resonant high-pass. Oscillator gain feeds the nonlinear stages; output gain follows them. Its output stage also clips the summed signal before applying the amp envelope. These are local measured approximations, not certified native-circuit models.

## Voice and gain structure

Proposed voice path:

```text
note pitch → C/A/B1/B2 phase generators → selected FM routing
          → X/Y audio taps → X/Y mix + source gain
          → Drift Type I / Type II → Drift high-pass
          → Drift summing saturation → amp envelope → volume/pan
```

Expose independent **Level** controls for C, A, B1 and B2 directly in their operator rows. Level is a dB gain applied to that operator's outgoing modulation and audio taps; an operator serving both roles changes both. Keep raw feedback taps separate so the Feedback control retains its defined response. The A/B group depth controls live only in the shared center editor alongside their envelopes. **Feedback**, **X/Y**, and **Source gain** remain separate controls. Source gain changes how hard the mixed audio hits the filter without changing upstream FM depth. Output volume stays downstream and changes listening level without changing distortion. Do not normalize every algorithm or insert automatic makeup gain: either would obscure intentional gain staging. Establish a documented oscillator reference scale, starting from Drift's 0.4 source scale, then calibrate the complete voice by ear and measurement.

Retain the shared B envelope and B depth mapping. B1/B2 tuning is independently editable, avoiding the need to cycle a hardware encoder through ratio pairs. Show ratio and fine offset directly for all four operators. Preserve phase-reset selection, envelope reset behavior, and key scaling; use descriptive units rather than copying undocumented 0–127 response curves. Fixed-Hz tuning is a possible later Operator-inspired extension, not required for the first version.

Extract the reused Drift filter/summing functions into a shared factory DSP source through supported compiler imports, keeping their numerical behavior intact. Verify the import/dependency packaging before moving them. If the current content/compiler boundary cannot support this cleanly, fix that support rather than using an untracked include or silently divergent copies.

## Envelopes and release

The reference has two timbre envelopes: A and shared B. They offer delay, attack, decay, an end level, triggered/gated behavior, and reset/retrigger control. Triggered mode proceeds through attack and decay; gated mode holds the peak while the note is held, then decays toward End. End may remain nonzero while the amp releases. [Elektron operator envelopes](https://support.elektron.se/support/solutions/articles/43000566573-operator-envelopes)

Implement those as explicit per-voice state machines. Add **Hold on release** as a separate, clearly named option: on note-off, capture the current timbre-envelope value and hold it for the amp tail. This differs from simply letting the reference envelope continue toward End. Default Hold on for the proposed init sound, preserving the user's preference; turning it off restores continuing triggered/gated behavior. A note-off during delay or attack captures the current value, not an assumed sustain level. External modulation of FM depth continues to work while the envelope is held.

The amp ADSR owns audible termination. New notes/retriggers must honor reset policy without inheriting a previous voice's hold latch or feedback state accidentally. Release-time edits must not depend on a fixed timeout. Check silence retirement, voice stealing, short notes, overlapping notes, and note-off during every stage. Keep a dedicated filter envelope for ordinary note articulation; do not require a host modulator slot merely to make a pluck.

## Compact interface

Operator's surrounding controls remain visible while its center changes with selection. Borrow that interaction and the screenshot's density. [Ableton Operator overview](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator)

Three regions, one fixed-height instrument surface:

The host owns the instrument header. Do not repeat the synth name inside the custom UI or add taglines, explanatory footers, "layout study" text, or development-status copy. The body contains functional controls and visual feedback only.

- **Left, about 30%:** exactly four shallow rows, B2/B1/A/C, each with identity, Ratio, Fine and Level (dB). All four levels are always visible and independently editable. Nothing is placed below these rows: available vertical space is a hard constraint. Selecting B1 or B2 opens the same B envelope, explicitly labeled as shared. Do not invent four independent envelopes in the UI.
- **Center, about 42%:** one continuous theme-accent surface. Overview shows eight algorithm miniatures and the selected routing, feedback and X/Y controls. Selecting A/B opens its envelope plot, group depth and numerical controls; C opens tuning/phase details. Filter and Amp use this same editor. Harmonics occupies a center page with a morph control and waveform/partials view; it adds no height or extra strip to the operator column.
- **Right, about 28%:** compact Filter, Amp, and Output rows: filter type/cutoff/resonance/high-pass; amp attack/release; source gain/output volume. Fine settings appear in the center. No permanent LFO, pitch-envelope, effects, or modulation-matrix panels.

Use small knobs only for continuously played controls; number pickers and short choices for tuning, times and modes. Avoid repeated headers, nested cards, giant envelope controls and decorative spacers. Keep full parameter names available to accessibility/tooltips. Theme the center using existing control-on surface/foreground tokens; operator identity can use small labeled accents without changing the whole editor's palette.

Reuse scoped parameter widgets, modulation wrappers, p-lock indication, and per-parameter reactive bindings. Selection changes only the detail subtree. Envelope diagrams must depict ADE/ASDE/hold behavior honestly; do not label a stock ADSR picture as one of these modes. A new diagram prop must be registered as bindable and tested as a ReactiveRef.

## Host modulation

Expose every continuous sound parameter with explicit bounds, units and depth ranges: ratios/offsets, individual operator levels, A/B depth, harmonics, feedback, X/Y, source gain, cutoff/resonance/high-pass, envelope times/end levels, filter-envelope amount, keytrack, output/pan. Keep `@mod` declarations top-level and pass `(mod parameter)` into section macros, following the verified factory convention.

Do not equate modulation metadata with implementation: verify that a real host modulator changes each destination. Smooth controls in their intended domain; use phase-continuous frequency changes. Quantize discrete choices explicitly. Algorithm and filter-type changes need a defined, tested transition; never interpolate integer algorithm IDs into fractional graph edges. Do not advertise audio-rate algorithm modulation before a correct transition design exists. Ordinary stepped automation remains part of the acceptance scope.

Keep intrinsic A/B/amp/filter envelopes. Omit internal general LFOs, a modulation matrix, sequencer, arpeggiator and send effects; the host supplies those.

## Harmonics: included in the first version

Digitone uses interpolated additive spectra, with negative Harm affecting C and positive Harm affecting A/B1. The sequence includes saw, reduced saw, odd/even mixtures, square, reduced square and bell-like spectra. [Elektron harmonics](https://support.elektron.se/support/solutions/articles/43000566560-harmonics)

User decision: additive spectral morph belongs in the first version. Develop the sine-only core as a validation baseline, then include the completed morph in the same factory release. We will author our own documented spectral recipe rather than require exact Elektron coefficients or claim a bit-exact clone.

Use a bipolar **Harmonics** control: zero is pure sine; negative values shape C; positive values shape A and B1, leaving B2 sine-based. This preserves the reference's useful division between changing the audible carrier and enriching the modulation source. Expose the control to host modulation and p-locks. Put its waveform and partial-amplitude display in the shared center editor.

Define explicit harmonic-amplitude vectors for a small set of anchor spectra: sine, saw-like, reduced saw, odd/even blend, square-like, reduced square, and a sparse bell-like spectrum. Interpolate adjacent coefficient vectors continuously, using aligned partial phases and a fixed fundamental amplitude. The harmonic-only bell anchor is a spectral color; inharmonic bells also use FM ratios. Keep the anchor data and interpolation rule readable and independently testable. Do not normalize in response to notes or modulation: extra partials may add energy and push the Drift filter harder, while the user's Source and Volume controls retain their separate roles.

The resulting periodic waveform replaces the sine lookup inside each affected operator: evaluate every partial at the operator's phase after incoming phase modulation. Shaping only an unmodulated side signal, or applying a waveshaper after the complete FM voice, would produce different behavior. The old experiment's self-PM formula is not this design.

Direct partial summation is the correctness reference. Benchmark it against precomputed, band-limited tables derived from the same coefficient vectors before choosing the production renderer. Smooth transitions between spectral anchors and bandwidth levels; do not rebuild tables or allocate on the audio thread. Pitch-based partial limiting alone does not suppress FM-generated sidebands, so evaluate deep modulation and feedback against a high-rate reference and choose an explicit anti-aliasing policy. Partial count, table resolution and oversampling factor are measurement decisions, not promises made by this proposal.

Acceptance requires a pure sine at zero, coefficient/spectrum agreement at anchors and intermediate positions, the correct C versus A/B1 assignment, continuous morph sweeps including zero crossings, stable fundamental tuning, block-size consistency, real host modulation, and complete-voice CPU/aliasing checks. Include presets that sweep Harmonics during held notes and amp-release tails. The manual does not supply exact coefficients or DSP internals; this limits claims of Elektron equivalence, not our ability to implement additive morphing cleanly.

## Quality gates for implementation

Use same-sample feedforward evaluation in each algorithm's topological order, with explicit causal state for feedback only. Define feedback delay and filtering as our own DSP design unless measured against hardware. Validate aliasing at high notes, deep modulation and feedback; sine oscillators alone do not make FM alias-free. Compare against a high-rate reference before deciding the production rate and oversampling policy. Measure complete-voice CPU and polyphony, not just an oscillator microbenchmark.

Required validation covers the eight routing definitions and output tap gains, envelope stage/retrigger/hold behavior across block sizes, X/Y endpoints, nonlinear input gain versus linear output gain, host modulation/p-lock/preset behavior, finite samples and release retirement. Run `instrument_probe` through the production host path. Add narrow factory UI layout tests for working controls and nonzero finite geometry, plus a `metal_seq capture` fixture for each center page and inspect the resulting PNGs. Use nextest exact selections; no full suite is justified by this design task.

Suggested sound palette: electric keys, woody bass, soft digital pad, glass bell, metallic pluck, and a driven mono lead. Include long-release examples specifically to demonstrate preserved FM character.

## Implemented version

Factory content now lives in `content/instruments/Synths/Digi FM`, with six presets in the adjacent bank. `tools/digi-fm/build.py` owns the eight graphs and seven 16-partial spectral anchors; `build_ui.py` derives graphics from that data. Authored UI controls and envelope/filter macros stay separate from generated routing arithmetic.

The renderer uses interpolated cumulative harmonic tables, four explicit phase/feedback substeps, and an eighth-order Butterworth decimator. The checked-in `spectra.json` contains 112 periodic waveforms of 8192 samples: seven spectral anchors for each of 16 partial limits. Interpolation across phase, anchor and partial limit retains the additive coefficient recipe and fractional bandwidth fade. B2 remains a direct sine. The fundamental remains fixed at amplitude one. Frequencies are bounded below the internal Nyquist; coefficient bandwidth limiting and oversampling reduce aliasing but do not eliminate deep-FM/feedback aliasing. This is our own DSP, not a bit-exact Elektron emulation.

The optimized renderer was promoted from the user-approved Digi FM Fast Test on 2026-09-23. Its generated DSP expressions and table are identical to that auditioned version. Table interpolation can diverge from the original additive renderer in feedback-sensitive sounds, especially algorithm 5; this promotion preserves the approved Fast Test sound, not bit identity with the older additive factory renderer. The bank occupies 3.5 MiB of float32 tensor data per voice with the current host allocation model.

Algorithm changes fade out over 2 ms, switch the complete graph and clear feedback/decimation state at zero, then fade in over 2 ms. This intentionally creates a short dip during live routing changes. Eight literal algorithm calls specialize the state-free core behind execution gates; phase, feedback and decimator histories stay outside those gates. Filter type smoothly blends the nonlinear filters, then freezes the unused topology once its contribution falls below 0.0001. Filter changes therefore differ from an always-running pair. The original Drift continues to use its original hard selector and matches its prior output in the parity test.

Validation on this Apple Silicon host at 48 kHz: all eight graphs at five bipolar spectral positions match an independent graph evaluator that interpolates direct additive sums at the table's phase grid; 16/128-frame blocks match; note-off during attack/decay/end holds exactly through a 13-second observation; gated decay and retrigger pass; amp release reaches silence; output volume is linear after the nonlinear stage. Relative spectral magnitude error below 10 kHz versus a 192 kHz host reference was 0.012% for sine, 0.019% for keys, and 1.27% for a bright high-note patch. This excludes feedback because its deliberate one-substep delay changes with sample rate, and is not a universal alias rejection specification.

The original additive version measured 1.49 ms for eight complete voices per 128-frame batch against a 2.67 ms deadline. The promoted renderer measured 0.587 ms in that validator on 2026-09-23. This excludes host graph scheduling and other tracks/effects; it is not a guarantee of total project polyphony. See `docs/digi-fm-performance-2026-09-22.md` for alternating native benchmarks and the approval/qualification history.

The production probe loads all six presets and verifies real Harmonics modulation equals the same directly set value. Its instrument path now shares the effect probe's support for modulation descriptor overrides. Targeted UI tests cover all seven pages, every parameter binding, and finite visible geometry; all page captures were inspected. Final voicing remains subjective and should be auditioned in a real project. No runtime workaround or undocumented external DSP dependency was introduced.


The A/B graph now uses the built-in `adsr-editor :mode :ade`, replacing the
original read-only custom contour. Delay/Attack/Decay/End handles share the
standard editor's drag and release-commit lifecycle. See
`docs/adsr-editor-modes.md` for the shared widget contract.

The Harmonics bars bind directly to the host's `mod-value-field` through
`param-effective-value`. Engine modulation telemetry updates the existing shader
state at the host display rate, while the Harmonics control continues to edit
the base value. The graph uses the same spectral anchors as the DSP; it shows
the harmonic source shape, not an FFT of the final FM/filter output.
