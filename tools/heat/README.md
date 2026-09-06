# Heat reference measurements

These tools compare original Heat DSP with isolated captures from the installed
Ableton Live 12.4.5 Analog device. Component measurements do not constitute a
finished-synth sonic-match verdict. See `docs/heat-synth-spec.md` for the full
acceptance contract.

`make_reference.py` builds disposable Live sets from an explicitly saved copy.
It preserves the input, replaces copied clips and automation, records every
native parameter, and supplies numeric MIDI note events at 120 BPM. Export all
individual tracks from Live at **48 kHz, stereo, 32-bit float**, for **10 bars
(20 seconds)**. Disable normalization, loop rendering, MP3, analysis files, and
return/main effects. The exporter remembers its duration separately from the
set: check the duration every time. The analyzer rejects wrong-length renders.

Raw sets, the preserved user-session copy, and WAVs live in the ignored
`reference/` directory. Keep them locally; do not include that directory in a
factory bundle. `measurements/` contains compact, hash-attributed results.
`initialized-parameters.json` is a separate browser-loaded default device,
not an assumption that the user's inspected patch was initialized.

The Python tools require NumPy and SciPy. Point the audition harness and host
probe at the **same explicit compiler and toolchain**:

```sh
export ESEQ_DGENLISP_TOOL=/absolute/path/to/DGenLisp
export ESEQ_DGEN_TOOLCHAIN_ROOT=/absolute/path/to/dgen-toolchain
export DGEN_BINARY_AUDIT_TOOL=/absolute/path/to/audit-dgen-dylib.sh
export PYTHONDONTWRITEBYTECODE=1
python tools/heat/check_filters.py tools/heat/reference \
  --out tools/heat/measurements/compiled-filter-analysis.json
python tools/heat/check_envelopes.py tools/heat/reference \
  --out tools/heat/measurements/compiled-envelope-analysis.json
python tools/heat/check_formants.py tools/heat/reference \
  --out tools/heat/measurements/compiled-formant-analysis.json
python tools/heat/check_drives.py tools/heat/reference \
  --out tools/heat/measurements/compiled-drive-analysis.json
python tools/heat/check_lfo.py tools/heat/reference \
  --out tools/heat/measurements/compiled-lfo-analysis.json
cargo run -p sequencer --bin instrument_probe -- \
  /absolute/path/to/eseq/tools/heat/probes/contour-filter.lisp \
  --sample-rate 48000 --frames 48000 --gate-frames 24000 \
  --min-peak 0.01 --min-rms 0.001 --json
```

The audition cache includes the compiler's SHA-256. Reports record that hash;
changing compiler builds invalidates cached DSP. The host probe also resolves
the factory macro imports and validates the compile/load/init/render path.

The linear-filter comparison feeds the independent source capture through the
compiled filter, then compares complex harmonic coefficients without fitted
gain or phase correction. Its current coverage is eight modes at 833.782 Hz
and two measured Q settings. The contour comparison covers both slopes and all
loop/Free combinations, plus six A/D/R settings from 5 ms to about 2.06 seconds.
It uses a separate sustained sine as its amplitude reference, identical
measurement smoothing, and a paired raw-sample comparison between slopes to
cancel common oscillator onset behavior without a fitted gain or time shift.
The demodulated time-range check excludes the first 128 frames; the paired
check includes them. Oscillator onset itself remains a separate release gate.

The longer captures show that the displayed infinite sustain setting holds
for linear slope but continues a slow exponential fall for exponential slope.
The measured ten-second trajectory agrees with the normalized exponential
curve at a 1000-second duration. The endpoint at 1000 seconds has not been
captured. Heat's contour reproduces that measured trajectory.

The formant comparison validates three parallel bandpass resonators across
nine cutoff settings. F6 identification supplies centers and one set of signed
weights; F12 is independently predicted with Q changing from 20 to 45 and
weights scaled by two-thirds. Native vowel quantization and the exact
control-to-center-frequency mapping are still unresolved. The macro exposes
physical center frequencies and signed weights, not a guessed vowel control.

The drive-steady and drive-independent corpora isolate the six drive modes
with a constant amplifier envelope. The compiled rational soft-knee model
matches both symmetric and asymmetric modes at two resonance levels. At high
levels, Drive Off is also compressed by an output limiter (knee 2.8, ceiling 4
in recorded channel units). An independent quieter filtered capture and two
unfiltered level controls reconstruct the unclipped drive input. The drive
macro uses a unit knee; the comparison applies one identified amplifier/pan
scale, shared by every mode. This validates the transfer curves, not the
amplifier knob law or the limiter's placement relative to global volume.
None of these component checks establishes oscillator, complete modulation,
full parameter-range, or complete-instrument parity.

Heat's LFO runs at audio rate. Matching Analog's internal control stepping and
interpolation is explicitly outside scope. The compiled timing checks cover
44.1, 48 and 96 kHz, phase retrigger on/off, repeated-note delay/fade restart,
and zero frequency with wrapped phase offsets. Reference waveform errors are
reported separately and remain provisional, particularly at extreme widths;
they are not pass/fail tests for reproducing the reference's control grid.

## Playable development voice

`instrument/dsp.lisp` now connects two analytical oscillators and sub sources,
colored noise, two independently routed filter/drive/amp lanes, four filter/amp contours, two pitch decays,
two LFOs, and pitch/filter/level pressure destinations. `instrument/ui.lisp`
provides the persistent overview and eight section views. `instrument.presets`
contains five fully specified development sounds. This remains outside the
factory content tree. Its source waveforms, level laws and modulation depths
are provisional; hard sync, glide, unison, vibrato, formant
control mapping and complete reference matching are not implemented here yet.

Run `tools/heat/run-development.sh` with the compiler/toolchain environment above
to open this checkout's app. It links the development source and preset bank
into this checkout's own Library as **Heat Development**, without replacing an
existing entry. The locally fixed Heat compiler is currently required; the
installed app/compiler distribution has not been updated. The launcher has been syntax checked and the development app has started
successfully with its audio callback and MIDI input active. The custom panel
was verified separately through production headless captures and layout tests.

The compact UI follows Analog's upper/lower signal paths and central detail
display. Global/Volume is the initial view, with four clickable Quick Routing
presets. Their filter/amp switches and three source balances were checked against
the running Analog device. Each preset applies all eight parameters in one
undoable batch; the colored routing diagrams are Lisp `defwidget` SDFs.

Filter and amp details use the existing editable `adsr-editor`, with all four
handles bound to the corresponding DSP parameters through scoped ADSR callbacks.
It uses the standard editor's contour rendering; finite sustain, loop and slope
modes remain separate controls. Oscillator details extend the builtin editor
with a bipolar, two-handle decay mode. Initial pitch is in semitones and duration
in milliseconds. This finite k=5 exponential is a development law; Analog's
normalized depth/time mapping and measured contour matching remain release work.
It retriggers on physical note-on, including legato, and reaches exact zero after
a whole-sample duration. The default initial depth is zero in all five presets.

Both LFOs share one selectable panel and detail page, using the builtin
`lfo-curve` with two cycles visible. The widget now supports Heat's clipped
triangle, full-width pulse, random step and ramp previews. Random previews use a
repeatable illustrative sequence, not live voice randomness. Noise no longer
changes the selected section; its filter balance is in Global beside routing.
Semitone knobs use whole steps, while detune remains fine-grained. Quick Routing
uses a persistent yellow border and tinted fill for the matching configuration;
manual routing combinations can leave all four presets unselected.

All Heat label widgets use centered vertical alignment. The custom-UI namespace
fix from the earlier UI pass is required for the routing SDF macros. Rebuild
the app for the new builtin envelope and LFO widget capabilities.

`capture-oscillator.lisp`, `capture-lfo.lisp`, `capture-filter.lisp`, and
`capture-amp.lisp` select the other detail pages using the same production capture
command below. The development capture deliberately uses the initial Global view.

```sh
python tools/heat/check_pitch_envelope.py
python tools/heat/check_voice.py \
  --out tools/heat/measurements/compiled-voice-analysis.json \
  --demo /tmp/heat-development-demo.wav
cargo run -p sequencer --bin instrument_probe -- 'user:Heat Development' \
  --preset 'Open Pad' --frames 96000 --gate-frames 48000 \
  --min-peak 0.01 --min-rms 0.001 --json
cargo run -p sequencer --bin metal_seq -- capture \
  --script tools/heat/capture-development.lisp --buffer fx --track 0 \
  --width 2400 --height 420 --out /tmp/heat-development-ui.png
```

The voice checks verify serial/parallel routing, exact silence after release,
independent legato envelope policy, pressure level scaling, finite filter
extremes at 44.1/48/96 kHz, and preset completeness/audio. The output is float
WAV at its authored gain, without normalization. Extreme resonance can exceed
full scale; no concealed limiter is used to make a stability check pass.
