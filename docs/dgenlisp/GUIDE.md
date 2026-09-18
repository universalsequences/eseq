# DGenLisp

DGenLisp is the language eseq instruments and audio effects are written in. A
program is a dataflow graph over per-sample signals. It compiles to C, then to a
native library that the audio thread calls once per block, and every voice of a
polyphonic instrument runs the same program with its own state.

There are no loops, no conditionals that branch, no runtime allocation, and no
functions in the usual sense. There are signals, constants, tensors, history
cells for feedback, and macros that stamp out subgraphs. That is enough to write
a wavetable synth, a Schroeder reverb, a finite-difference drum membrane, or a
spectral freeze, and all four ship in `content/`.

This guide is for people and agents writing `dsp.lisp` files. The catalogue of
operators is generated from the compiler and lives in
`crates/sequencer/docs/dgenlisp-api.json`; when this guide and the manifest
disagree, the manifest wins.

## Hello, Sine

```lisp
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param attack @default 5 @min 0 @max 1000 @unit ms)
(param release @default 180 @min 1 @max 5000 @unit ms)
(param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

(def env (adsr gate trigger attack 120 0.8 release))
(def osc (sin (* (phasor pitch) twopi)))

(out (* osc env velocity (mod gain)) 1 @name audio)
```

Save it as `content/instruments/Synths/Hello/dsp.lisp` (or anywhere under your
library at `~/.eseq.d/instruments/`), pick it in the browser, and play a note.
A `(param …)` with no `ui.lisp` beside it becomes a knob on its own.

To hear it without the app:

```sh
python3 tools/audition/audition.py "content/instruments/Synths/Hello" \
  --pitch a2 --seconds 2 --wav /tmp/hello.wav
```

The nine `in` lines are the instrument contract: gate (0 or 1), pitch in Hz,
velocity 0..1, a one-sample trigger pulse, a 0..1 bar-phase clock, and four
modulation buses. They are bound by `@name`, not by position, and the four
`@modulator` inputs must exist whenever any param says `@mod true`.

## Core Features

### A file is a flat list of forms

There is no wrapper. No `(instrument …)`, no `(main …)`. A program is
top-level `def`, `defmacro`, `param`, `in`, `out`, and `make-history` forms,
in order.

Order matters. A symbol must be defined above the first form that reads it.
Forward references are a compile error: reorder the file.

```lisp
(def a (phasor 2))
(def b (* a 0.5))   ; fine: a is above
```

### Everything is a signal

A numeric literal is a compile-time `float`. Everything computed from an
input, a param, a generator, or a history cell is a per-sample `signal`.
Params are signals too, updated once per block. There is no distinction in
the source; `(* osc 0.5)` and `(* osc gain)` look the same.

Arithmetic is n-ary: `(+ a b c)`, `(* a b c d)`, `(- x)` negates. `min` and
`max` take any number of arguments. Comparisons `< > <= >= ==` (spelled
`lt gt lte gte eq` if preferred) return 0.0 or 1.0, and factory code uses them
as masks:

```lisp
(def rising (* gate (lte prev_gate 0.5)))   ; 1.0 for one sample on note-on
```

Remainder is `%`. `(mod name)` is not modulo. It is the modulation accessor
described below, and `(mod x y)` is a compile error.

### There is no `if`

`(gswitch cond a b)` picks `a` when `cond` is nonzero, otherwise `b`. Both
branches are always computed, so keep them cheap.

```lisp
(def env_next (gswitch (gt trigger 0.5) 1.0 (* prev coef)))
```

`(selector k opt1 opt2 …)` picks the k-th option, 1-based. `k <= 0` yields 0.
This is how an "engine" or "mode" param becomes a switch:

```lisp
(param engine @default 1 @min 1 @max 3)
(def voice (selector (clip (round engine) 1 3) sub_voice fm_voice noise_voice))
```

### Params

```lisp
(param cutoff @default 900 @min 40 @max 12000 @unit Hz)
```

Declares a knob named `cutoff`, then use `cutoff` as a signal. Every
non-hidden param shows up in the generated UI, is a p-lock target on every
step, and can be saved into a preset. `@unit` is a display hint.

Names are plain symbols; factory code uses `snake_case` with a unit suffix:
`amp_attack_ms`, `size_fine`, `lfo1_to_cutoff`. Never write dotted names.

A param may also sit inline where it is used:

```lisp
(def freq (param freq @min 0.1 @max 500 @default 32))
```

`@group` and `@env`/`@role` shape the generated panel: params sharing a
`@group` land in one block, and four params sharing an `@env` with the roles
`attack decay sustain release` become one envelope editor. `@group` prefixes
the param's host-facing name, so `(param pressure @group reed …)` is
`reed.pressure` in presets and in `ui.lisp`.

`@hidden true` keeps a param out of the UI.

### Modulation

Mark a param `@mod true`, give it a `@mod-mode`, and read it with `(mod name)`:

```lisp
(param cutoff @default 900 @min 40 @max 12000 @unit Hz @mod true @mod-mode additive)
(def filtered (svf osc (clip (mod cutoff) 40 12000) 1 0))
```

`(mod cutoff)` is `cutoff` plus the weighted sum of the four modulation
buses, clipped to the param's range. The depth per bus is a hidden generated
param, so it is itself p-lockable and appears in the Mod tab. Modes:

| mode | result |
|---|---|
| `additive` | `clip(base + m, min, max)` |
| `multiplicative` | `clip(base * (1 + m), min, max)` |
| `semitone` | `base * 2^(m/12)` |

Rules the validator enforces:

- `(mod p)` only on a param declared `@mod true`.
- If any param is `@mod true`, all four `mod1..mod4` inputs must be declared,
  with `@modulator 1..4`, before the params.
- Never read `mod1..mod4` directly in DSP. The buses feed `(mod …)`; that is
  their only job.
- Do not put `@mod true` on a selector or on a local depth param like
  `env_to_cutoff`. Modulate the target, not the amount.

The patcher lets you write `cutoff~` and infers `@mod true`. In a hand-written
`dsp.lisp`, spell `(mod cutoff)`.

### Feedback: history cells

A history cell holds one sample across frames. `read-history` returns the
previous frame's value, `write-history` sets this frame's value and returns it.

```lisp
(make-history h)
(def fb (read-history h))
(def y (write-history h (delay (+ input (* 0.4 fb)) 4800)))
(out y 1 @name audio)
```

That is a feedback delay with a 100 ms tap at 48 kHz. `delay` takes its time in
samples and the time may be a signal. The default ring buffer is 88000 samples;
`@max-delay N` raises it.

A one-pole smoother, the most common idiom in the tree:

```lisp
(make-history s)
(def coef (- 1 (exp (/ -1 (* 0.005 samplerate)))))
(def smoothed (+ (read-history s) (* coef (- target (read-history s)))))
(write-history s smoothed)
```

Fresh voices start with zeroed history, so a smoother sweeps up from 0 on the
first note. If that is audible, seed it with a second cell that flips to 1
after the first sample:

```lisp
(make-history prev)
(make-history ready)
(def value (gswitch (read-history ready) (mix target (read-history prev) coef) target))
(write-history prev value)
(write-history ready 1)
```

### Macros

`defmacro` defines a subgraph template. Each call site stamps out a fresh
copy, including its own history cells, so a macro that holds state is safe to
call many times.

```lisp
(defmacro onepole-lp (x hz)
  (make-history y)
  (def a (- 1 (exp (/ (* -2 pi hz) samplerate))))
  (write-history y (+ (read-history y) (* a (- x (read-history y))))))

(def warm (onepole-lp osc 2000))
(def warmer (onepole-lp warm 800))   ; a second, independent filter
```

The body is a sequence of `def`, `make-history`, and `write-history` forms;
the last expression is the value. Multiple values come back as a tuple:

```lisp
(defmacro stereo-spread (x amt)
  (tuple (* x (+ 1 amt)) (* x (- 1 amt))))

(def (l r) (stereo-spread voice 0.3))
```

A `param` inside a macro body is legal and is hoisted to top level by the host
before compiling, deduplicated by name. Factory synths use this to keep a
section's private knobs next to its code. Params that are `@mod true` should
stay at top level regardless.

Shared macros live in `content/defmacros/<name>/macro.lisp` and are imported
by name:

```lisp
(use-defmacro pitch-transpose)
(def detuned (pitch-transpose pitch 7))
```

`use-defmacro` is host metadata, not a compiler form: the host pastes the
macro in before calling the compiler, transitively, with local definitions
shadowing library ones.

### Envelopes and oscillators

The host injects a preamble of macros every program can use without importing:

```lisp
(adsr gate trigger attack_ms decay_ms sustain release_ms)
(adsrexp gate trigger a_ms d_ms sus r_ms attack_curve fall_curve)
(svf input cutoff q mode)              ; mode 0 LP, 1 BP, 2 HP, 3 notch, 4 peak, 5 AP
(ladder input cutoff res drive)        ; res 0..1
(polyblep_saw phase freq)
(polyblep_pulse phase width freq)
(wavetable-read table wave phase)
```

`svf`'s `q` is resonance with 0.5 meaning none, and its bandpass output has a
gain of `q`; divide by `q` for a unity-gain band. `biquad` is the other filter:
`(biquad sig cutoff q gain mode)` with mode 0 LP, 1 HP, 2 BP. Its coefficient
math assumes 44.1 kHz, so factory drums scale the cutoff by
`(/ 44100 samplerate)`.

`(phasor freq)` is a 0..1 ramp. `(triangle phase duty)` shapes it, and
`(sin (* phase twopi))` makes a sine. `(noise)` is uniform 0..1 white noise.
`(latch value trigger)` samples and holds, which is the per-note random idiom:

```lisp
(def drift (latch (noise) trigger))
```

`(accum inc reset min max)` integrates, and `(accum (/ 1 samplerate) trigger 0 1e6)`
is the standard "seconds since the last hit" clock that drum patches build
segments on.

Constants: `pi`, `twopi`, `tau`, `e`, `samplerate`, `true` (1.0), `false` (0.0).

### Effects

An effect reads stereo audio and writes stereo audio. The names are checked:

```lisp
(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param rate @min 0.1 @max 20 @default 5)
(param depth @min 0 @max 1 @default 0.8)

(def p (phasor rate))
(def lfo (scale (triangle p 0.5) -1 1 (- 1 depth) 1))

(out (* in_l lfo) 1 @name left)
(out (* in_r lfo) 2 @name right)
```

Effects that modulate declare `mod1..mod4` on channels 3 to 6, and a
sidechain input, when wanted, comes after them:

```lisp
(def mod1 (in 3 @name mod1 @modulator 1))
…
(def sidechain (in 7 @name sidechain))
(def ducked (compressor in_l (mod ratio) (mod threshold) 6 .01 .01 1 sidechain))
```

An effect that introduces fixed latency declares it so the host can compensate:

```lisp
(effect-latency (+ 31 (round (* samplerate 0.001))))
```

The expression is evaluated by the host, not the compiler. It may use numbers,
`samplerate`, and basic arithmetic. Params are not allowed.

### Tensors

A tensor is a static array with a compile-time shape.

```lisp
(def bank (tensor @shape [512 512] @file "waves/bank.json"))
(def wave (wavetable-read bank (clip pos 0 511) (phasor pitch)))
```

The JSON asset is `{"shape": [512, 512], "data": [...]}`, row-major, and the
path is relative to the `dsp.lisp` folder. Tensor data is baked into the
compiled library; edit the JSON and recompile.

Inline data uses brackets without commas:

```lisp
(def laplacian (tensor @shape [3 3] @data [0 1 0  1 -4 1  0 1 0]))
```

Elementwise math broadcasts a scalar over a tensor. Reductions collapse to a
scalar: `(sum t)`, `(mean t)`, `(max-axis t @axis 0)`. `(sample t phase)`
reads a 1-D tensor at a normalized 0..1 phase with interpolation. `(gather t
idx)` reads by index and truncates fractional indices, so interpolate by hand
when that matters.

Recurrent tensor state uses the tensor form of history:

```lisp
(make-tensor-history p @shape [6 6])
(def p_prev (read-tensor-history p))
(def p_next (+ p_prev (* c (conv2d p_prev laplacian @padding same))))
(write-tensor-history p p_next)
```

Read each tensor history exactly once per frame and bind it to a `def`.

`(phasor <tensor>)` does not keep state across blocks. Use
`(stateful-phasor <tensor>)` for a bank of oscillators.

### Spectral processing

The STFT is spelled out; nothing is hidden.

```lisp
(def in_l (in 1 @name left))
(def win (sqrt (hann 1024)))

(def frame (* (reshape (buffer in_l 1024 512) @shape [1024]) win))
(def (re im) (fft frame @N 1024 @backend accelerated))
;; … per-bin math on re and im …
(def wet_frame (ifft re im @N 1024 @backend accelerated))
(def wet (overlap-add (* wet_frame win) 512))
(out wet 1 @name left)
```

`buffer` collects samples into a ring, `fft` returns `(real imag)`,
`polar-fft` converts to `(magnitude phase)`, and `overlap-add` resynthesizes.
The hop must divide the host block, and factory content uses hop 512.

Three rules keep spectral code fast and correct:

- **Hold params at hop rate before they touch a tensor.** A per-sample scalar
  demotes every tensor expression it touches to per-sample execution, which
  turns an 86 Hz IFFT into a 44100 Hz one. Write
  `(def width_h (hop-hold width 512))` and use `width_h`.
- **Magnitude masks must be conjugate-symmetric.** A gain array must satisfy
  `gain[k] == gain[N-k]` or the notches smear. Build bin-index tensors folded
  about N/2.
- **`write-history` on a tensor cell returns nothing useful.** Write it as a
  statement and hop-hold the expression you meant to keep:

  ```lisp
  (make-history mag_h @shape [1024] @hop 512)
  (def next_mag (max inject (* (read-history mag_h) 0.9)))
  (write-history mag_h next_mag)
  (def held (hop-hold next_mag 512))
  ```

Two identical `(noise @size N @hop H)` expressions compile to one node and
return the same values. To get independent noise, draw one wider tensor and
`gather` each consumer from a disjoint index range:

```lisp
(def nz (noise @size 4096 @hop 512))
(def n_a (gather nz idx))
(def n_b (gather nz (+ idx 2048)))
```

The host block is at most 512 frames; spectral buffers are sized for that.

### Polyphony, voices, and the clock

An instrument is compiled with 12 voices; the track's voice count decides how
many are active. Each voice owns its history cells, phasors, and delays.
Nothing in the source refers to voices. A patch that should ring past its own
note-off gates the tail with the envelope rather than the input, as Revsynt
does, and the host handles the rest.

`clock` is a transport-synced 0..1 ramp over one bar. It resets on start and
seek and holds 0 while stopped. Use it for tempo-locked LFOs:

```lisp
(def bar_lfo (sin (* clock twopi 4)))   ; four cycles per bar
```

An instrument that should sound with no note at all, such as a drone or a
generative patch, ships `instrument.json` beside its `dsp.lisp`:

```json
{"version": 1, "run_mode": "free_patch"}
```

### The folder

```
content/instruments/Synths/Hello/
  dsp.lisp           ; required; a folder is an instrument iff this exists
  ui.lisp            ; optional custom panel (eseqlisp, see the eseqlisp guide)
  instrument.json    ; optional run mode
  dsp.layout.json    ; patcher node positions, not semantic
  waves/bank.json    ; assets, referenced relatively
content/instruments/Synths/Hello.presets   ; preset bank sits BESIDE the folder
```

A preset is a sparse map from param name to value; anything unlisted keeps its
default. Effects use the same layout under `content/effects/<name>/`.

To share an instrument, put it in your library, then **File > Export
Package**. The export inlines any `use-defmacro` imports and warns about
absolute asset paths, which will not exist on the recipient's machine.

## Gotchas

- **No forward references.** Define before use, always.
- **`(mod x)` is modulation, `%` is remainder.**
- **`clamp` does not exist.** It is `clip`. Comments in the old templates
  mention `clamp`, `history`, `onepole`, and `(read-history h n)`; none of
  those are operators. Trust the manifest.
- **Fresh voices start at zero.** Every history reads 0 on the first sample of
  a voice. Seed smoothers and octave multipliers deliberately.
- **`(mod p)` is already clipped** to `[min, max]`, but factory code still
  clips at the use site so that a later range change cannot push a filter
  unstable.
- **`selector` is 1-based**, and pass it an integer: `(selector (+ 1 k) …)`.
- **`gate` and `trigger` are different.** Gate is high while a key is down;
  trigger is a single-sample pulse. Envelopes that should retrigger on legato
  take `(max gate_rising trigger)`.
- **`biquad` thinks it is at 44.1 kHz.** Scale the cutoff.
- **`gather` truncates.** Interpolate between floor and floor+1 yourself.
- **`floor` was a no-op** before compiler v0.1.6; the pinned compiler is fine,
  but `(- x (% x 1))` in older files is that workaround, not a style choice.
- **Assets are baked in.** Changing a JSON tensor means recompiling.
- **Presets and `ui.lisp` use the grouped name** (`reed.pressure`) when a param
  has `@group`.
- **Unknown param attributes are dropped silently.** A typo like `@defualt`
  compiles and leaves the default at 0.
- **The standalone auditioner does not hoist.** `tools/audition/audition.py`
  injects the preamble but not `use-defmacro` or macro-internal params. Inline
  them, or go through the app.

## Tooling

The compiler and its clang/lld stage are fetched, not tracked:

```sh
./scripts/fetch_dgenlisp.sh          # pinned by content/dgenlisp.lock
./scripts/fetch_dgen_toolchain.sh    # pinned by content/dgen-toolchain.lock
```

`ESEQ_DGENLISP_TOOL=/abs/path` points at a locally built compiler;
`ESEQ_DGEN_TOOLCHAIN_ROOT` at a different stage.

Render an instrument to a WAV, with param ramps and retriggers:

```sh
python3 tools/audition/audition.py "content/instruments/Drums/808 Clap" \
  --set decay=2 --ramp bright=0:1,1:4 --retrig 0,0.5,1 --seconds 3 --wav /tmp/clap.wav
python3 tools/audition/audition.py "content/instruments/Drums/808 Clap" --list-params
```

Compile an effect and print the host manifest, including latency:

```sh
cargo build -p sequencer --bin dgen_effect_compile
target/debug/dgen_effect_compile /abs/path/dsp.lisp --sample-rate 48000
```

The compiled output is `<name>.dylib` plus `<name>.json`. The manifest lists
every param with its memory cell; a host drives the library by writing floats
into `memory[cellId]` and calling `process` in blocks of at most 512 frames.
Write every default before the first block, or the patch runs on zeros.

Render the panel without the app: see the `render-panel` skill and
`metal_seq capture`.

## Syntax Reference

Comments start with `;`. Attributes are `@name value` after the positional
arguments. Arrays are bracketed, whitespace-separated: `@shape [512 32]`.
Ranges are `start:end`, comma-separated: `@ranges [0:2,1:3]`.

### Top-level forms

```lisp
(def name expr)                     ; bind
(def name expr1 expr2 …)            ; bind the last expression
(def (a b …) tuple-expr)            ; destructure
(defmacro name (params…) body…)     ; subgraph template; last form is the value
(param name @default v @min v @max v [@unit s] [@group g] [@env e @role r]
            [@hidden true] [@mod true @mod-mode m @mod-depth-min v @mod-depth-max v])
(in ch @name sym)                   ; 1-based channel
(in ch @name modN @modulator N)     ; modulation bus, N in 1..4
(out expr ch @name sym)             ; signal first, channel second
(make-history name)                 ; scalar cell
(make-history name @shape [d…] [@hop N] [@data […]])
(make-tensor-history name @shape [H W] [@data […]])
(use-defmacro name)                 ; host: paste content/defmacros/<name>
(effect-latency const-expr)         ; host: effects only
```

### Special forms and stateful operators

```lisp
(read-history h) (write-history h expr)
(read-tensor-history h) (write-tensor-history h expr)
(mod param)                          ; modulated value, @mod true required
(tuple a b …)
(gswitch cond a b)   (selector k o1 o2 …)   (block-gate cond body)
(accum inc)  (accum inc reset min max)
(latch value trigger)  (event-hold value trigger)  (hop-hold value hop)
(mix a b t)                          ; lerp
(delay sig samples)  (delay tensor samples)  (delay tensor times @max-delay N)
(poke tensor index value)  (seq first second …)
```

### Math

```
+ - * / %   min max   abs sign floor ceil round
sin cos tan atan atan2 tanh   exp log log10 pow sqrt   sigmoid relu mse
eq == gt > gte >= lt < lte <=
clip sig lo hi   scale sig inLo inHi outLo outHi   wrap sig [lo hi]   triangle phase [duty]
```

### Generators and filters

```lisp
(phasor freq [reset])  (stateful-phasor freq [reset])  (noise [@size N @hop H])
(click)  (ramp2trig ramp)
(biquad sig cutoff q gain mode)          ; 0 LP 1 HP 2 BP
(compressor sig ratio threshold knee attack release [sidechain])
;; preamble macros:
(adsr gate trig a_ms d_ms sus r_ms)  (adsrexp … attack_curve fall_curve)
(svf sig cutoff q mode)  (ladder sig cutoff res drive)
(polyblep phase freq)  (polyblep_saw phase freq)  (polyblep_pulse phase width freq)
(wavetable-read table wave phase)  (wavetable-morph table a b phase morph)
```

### Tensors

```lisp
(tensor @shape [d…] @data […])  (tensor @shape [d…] @file "p.json")  (tensor @shape [d…])
(tensor-param @shape [d…] @name n [@default-file p | @data […]])
(audio-tensor @file "x.wav" [@channel c | @mono true] [@normalize peak] [@start s @end s])
(ir @file "room.wav")
(full [d…] v)  (ones [d…])  (zeros [d…])  (randn [d…])
(conv1d x k)  (conv2d x k @padding …)  (matmul a b)  (gather t idx)
(peek t i [ch])  (peek-row t row)  (sample t phase [ch])  (to-signal t [@max-frames N])
(expand t @shape […])  (pad t @padding […])  (repeat t @repeats […])
(reshape t @shape […])  (shrink t @ranges […])  (transpose t [@axes […]])  (windows t @shape […])
(sum t [@axis a])  (mean t [@axis a])  (sum-axis t @axis a)  (mean-axis t @axis a)
(max-axis t @axis a)  (softmax t @axis a)  (cumsum t @axis a)
```

### Spectral

```lisp
(buffer sig size [hop])  (hann N)  (window @type hann @N N)  (overlap-add t hop)
(fft frame @N n @backend accelerated)        ; -> (re im)
(ifft re im @N n @backend accelerated)
(polar-fft re im)  (rect-fft mag phase)  (complex-mul ar ai br bi)  (complex-conj re im)
(partition-ir ir @N n @hop h)  (partitioned-convolve sig ir @N n @hop h [@gain g])
(partitioned-spectral-mac xre xim irre irim @N n)
(phase-vocoder re im ratio @N n @hop h)
(spectrum-delay spec @N n @hops k @hop h)  (spectrum-delay-mod spec delay @N n @max-hops k @hop h)
```

### Constants

`pi twopi tau e samplerate true false`

## Further Reading

- `crates/sequencer/docs/dgenlisp-api.json`: the generated operator manifest.
- `crates/sequencer/docs/dgenlisp-modulation-mini-spec.md`: modulation lowering.
- `crates/sequencer/docs/dgenlisp-ui-metadata-spec.md`: how `@group`/`@env`/`@role` become a panel.
- `docs/tensor-asset-format-spec.md`: the JSON tensor format.
- `docs/instrument-audition-harness.md`: driving a compiled patch outside the app.
- `docs/custom-effect-latency.md`: `effect-latency`.
- Exemplars: `content/effects/stereo-tremolo`, `content/effects/spectral-stft-identity`,
  `content/instruments/Synths/Revsynt`, `content/instruments/Synths/Digi Wave`,
  `content/instruments/Drums/Membrane Snare`, `content/instruments/Physical Models/PM Clarinet`.
- The eseqlisp guide, `docs/eseqlisp/GUIDE.md`, for the `ui.lisp` sidecar.
