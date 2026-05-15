# DGenLisp

A Lisp-to-dylib compiler for DGen. Write DSP patches as S-expressions, compile to optimized native shared libraries with a JSON manifest.

## Usage

```
dgenlisp compile [<file.lisp>] [options]
```

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `-o`, `--output <dir>` | Output directory | `.` |
| `--name <name>` | Output file name (without extension) | `patch` |
| `--sample-rate <rate>` | Sample rate in Hz | `44100` |
| `--max-frames <count>` | Maximum frame count per process call | `4096` |
| `--asset-base <dir>` | Base directory for relative tensor/wavetable files | input file directory |
| `--debug` | Print debug information to stderr | off |
| `-` | Read from stdin | default if no file |

### Output

- `<name>.dylib` — Compiled shared library exporting `process()` and `setParamValue()`
- `<name>.json` — Manifest with params, I/O, memory layout (also printed to stdout)

## Language Reference

### Comments

```lisp
; line comment
# also a line comment
```

### Atoms

Numbers, symbols, and named constants:

```lisp
440           ; integer
3.14159       ; float
freq          ; symbol (must be defined with def or param)
pi            ; π
twopi         ; 2π (alias: tau)
e             ; Euler's number
true          ; 1.0
false         ; 0.0
```

### Special Forms

#### def — bind a name

```lisp
(def name expr)
(def osc (sin (* (phasor 440) twopi)))

; Destructure tuple-returning operators.
(def (re im) (fft frame @N 1024))
(def (mag phase) (polar-fft re im))
```

#### defmacro — define a reusable macro

```lisp
(defmacro name (params...) body...)

(defmacro ap (sig g d)
  (make-history h)
  (def ds (delay (read-history h) d))
  (def v (+ sig (* g ds)))
  (write-history h v)
  (- ds (* g v)))
```

Local `def` and `make-history` bindings inside macros are automatically scoped — multiple calls to the same macro won't collide.

#### History feedback

```lisp
(make-history name)         ; create a feedback cell
(read-history name)         ; read previous frame's value
(write-history name expr)   ; write current frame's value (returns expr)
```

#### Tensor history feedback

```lisp
(make-tensor-history name @shape [32 32])
(make-tensor-history name @shape [32 32] @data [0 0 ...])
(read-tensor-history name)
(write-tensor-history name expr)
```

Tensor history stores `tensor` or `signalTensor` values and returns a `signalTensor`
when read. It is useful for recurrent tensor DSP such as membranes, image-style
feedback, and physical models.

### I/O

#### param — host-controllable parameter

```lisp
(param name @default value @min value @max value @unit string)

(param freq @default 440 @min 20 @max 20000 @unit Hz)
(param gain @default 0.5 @min 0 @max 1)
(param cutoff @default 2400 @min 60 @max 12000 @unit Hz @mod true @mod-mode additive)
```

The name becomes a symbol you can use in expressions. Parameters appear in the manifest with their physical memory cell ID for host-side control.
Modulatable params generate hidden modulation source/depth params plus `modDestinations` metadata in the manifest.

#### in — audio input channel

```lisp
(in channel @name string)

(in 1 @name signal)     ; channel 1 (1-indexed)
(in 5 @name mod1 @modulator 1)
```

`@modulator <slot>` marks an input as a host-visible modulation source.

#### out — audio output channel

```lisp
(out expr channel @name string)

(out (sin (* (phasor 440) twopi)) 1 @name audio)
```

At least one `out` is required. Channel numbers are 1-indexed.

### Arithmetic

Binary operators auto-nest for 3+ arguments: `(+ a b c)` becomes `(+ (+ a b) c)`.

```lisp
(+ a b)      ; addition
(- a b)      ; subtraction
(- a)        ; negation
(* a b)      ; multiplication
(/ a b)      ; division
```

All arithmetic respects type promotion:

| Left | Right | Result |
|------|-------|--------|
| signal | signal | signal |
| tensor | tensor | tensor |
| signal | tensor | signalTensor |
| signalTensor | signal | signalTensor |
| signalTensor | tensor | signalTensor |
| any | float | promotes float |

### Math Functions

#### Unary

```lisp
(sin x)      (cos x)      (tan x)      (atan x)
(tanh x)
(exp x)      (log x)      (sqrt x)     (abs x)
(sign x)     (floor x)    (ceil x)     (round x)
(relu x)     (sigmoid x)  (log10 x)
```

Work on signal, tensor, signalTensor, and float.

#### Binary

```lisp
(pow base exponent)
(min a b)
(max a b)
(atan2 y x)
(% a b)
(mse prediction target)    ; mean squared error
```

`min` and `max` auto-nest like arithmetic operators.

### Comparison

Return 1.0 for true, 0.0 for false:

```lisp
(gt a b)     ; a > b
(lt a b)     ; a < b
(gte a b)    ; a >= b
(lte a b)    ; a <= b
(eq a b)     ; a == b
```

### Signal Generators

```lisp
(phasor freq)              ; ramp 0→1 at freq Hz
(phasor freq reset)        ; with reset trigger
(stateful-phasor freq)     ; forced stateful variant
(noise)                    ; white noise
(noise @size 1024)         ; per-bin signalTensor noise
(noise @size 1024 @hop 256); hop-rate tensor noise
(click)                    ; impulse: 1.0 on frame 0, then 0.0
```

`phasor` with a tensor frequency returns a signalTensor (one phasor per element).

### Stateful Operations

```lisp
(accum increment)                       ; accumulate, default range [0,1]
(accum increment reset min max)         ; with reset trigger and bounds
(latch value trigger)                   ; sample-and-hold
(mix a b t)                             ; linear interpolation: a*(1-t) + b*t
(hop-hold value hop)                    ; update value once per hop
```

### Audio Effects

#### biquad — IIR filter

```lisp
(biquad signal cutoff q gain mode)

; or with attributes:
(biquad signal @cutoff 1000 @q 0.707 @gain 0 @mode 0)
```

Modes: 0=lowpass, 1=highpass, 2=bandpass, 3=notch, 4=allpass, 5=peaking, 6=lowshelf, 7=highshelf.

#### compressor

```lisp
(compressor signal ratio threshold knee attack release)

; or with attributes:
(compressor signal @ratio 4 @threshold -20 @knee 6 @attack 0.01 @release 0.1)
```

Works on both signal and signalTensor.

#### delay

```lisp
(delay signal time_in_samples)
```

### Conditional

```lisp
(gswitch condition true_value false_value)
(selector mode option1 option2 ...)
```

`selector` is 1-based: `mode <= 0` returns `0`, `1` returns `option1`, `2` returns `option2`, and so on.

### Modulation

```lisp
(param cutoff @default 2400 @min 60 @max 12000 @unit Hz @mod true @mod-mode additive)
(def mod1 (in 5 @name mod1 @modulator 1))
(def filtered (biquad sig (mod cutoff) 0.8 1 0))
```

`(mod name)` resolves the generated modulated value for a parameter declared with `@mod true`.
Supported modulation modes are `additive`, `multiplicative`, and `semitone`.

### Utility

```lisp
(scale sig inMin inMax outMin outMax)  ; linear rescale
(triangle phase)                       ; phasor (0..1) → triangle (-1..1)
(wrap sig min max)                     ; wrap value to range
(clip sig min max)                     ; clamp value to range
```

### Tensor Creation

```lisp
(tensor rows cols)           ; alias for zeros
(zeros [d1,d2,...])          ; zero-filled tensor
(zeros d1 d2)                ; same, with individual dims
(ones [d1,d2,...])           ; all-ones tensor
(full [d1,d2,...] value)     ; filled with constant
(randn [d1,d2,...])          ; random normal
(tensor-param [d1,d2,...])   ; learnable parameter tensor
(wavetable @shape [512 32] @file "waves/factory.json")
(wavetable-param @shape [512 32] @default-file "waves/init.json")
(audio-tensor @file "irs/hall.wav" @mono true @normalize peak)
(audio-tensor @file "loops/stereo.wav" @channel 0)
(ir @file "irs/plate.wav")
(tensor @shape [4] @data [1 0.5 0.25 0])
```

`wavetable` and `wavetable-param` load JSON data relative to the source file, or
relative to `--asset-base` when provided. JSON may be a flat numeric array,
nested numeric arrays, or an object with `shape` and `data`. `peek` uses
`(index, channel)`, so wavetable banks usually use shape `[samples waves]`.
Flat wavetable data should store each channel/wave contiguously. Fractional
`index` and fractional `channel` values are interpolated, so 2D wavetable reads
are bilinear across sample position and wave position.

`audio-tensor` loads WAV files as static tensor data in the manifest. `ir` is the
same loader tagged as an impulse response. Relative audio paths resolve from the
input file directory, or from `--asset-base` when supplied.

### Tensor Operations

```lisp
(matmul a b)                           ; matrix multiply
(conv1d input kernel)                  ; 1D convolution
(peek tensor index)                    ; interpolated scalar at index
(peek tensor index channel)            ; bilinear scalar at (index, channel)
(peek-row tensor rowIndex)             ; read row → signalTensor
(sample tensor index)                  ; interpolated row read → signalTensor
(to-signal tensor)                     ; 1D tensor → signal via playback
(to-signal tensor @max-frames 4096)    ; with explicit frame limit
```

### Tensor Shape Operations

```lisp
(reshape tensor @shape [d1,d2,...])
(transpose tensor)                     ; reverse axes
(transpose tensor @axes [1,0])         ; specific axis permutation
(shrink tensor @ranges [0:2,1:3])      ; slice sub-tensor
(pad tensor @padding [1:1,0:0])        ; zero-pad (before:after per axis)
(expand tensor @shape [4,3])           ; broadcast expand
(repeat tensor @repeats [2,3])         ; tile/repeat
(conv2d input kernel)                  ; 2D convolution
(windows tensor @shape [3 3])          ; extract sliding windows / im2col view
```

### Reductions

```lisp
(sum tensor)                 ; sum all → scalar tensor
(sum tensor @axis 0)         ; sum along axis
(mean tensor)                ; mean all → scalar tensor
(mean tensor @axis 1)        ; mean along axis
(sum-axis tensor @axis 0)    ; explicit axis reduce
(mean-axis tensor @axis 0)
(max-axis tensor @axis 0)
(softmax tensor @axis -1)    ; softmax (tensor only)
```

### FFT

```lisp
(fft input)                  ; FFT, returns (real imag)
(fft input N)                ; legacy explicit size
(fft input @N 1024 @backend accelerated)
(ifft real imag)             ; inverse FFT
(ifft real imag N)           ; legacy explicit size
(ifft real imag @N 1024 @backend accelerated)
(polar-fft re im)            ; returns (magnitude phase)
(rect-fft mag phase)         ; returns (real imag)
(complex-mul ar ai br bi)    ; returns complex product (real imag)
(complex-conj re im)         ; returns (real -imag)
```

Preferred style is destructuring:

```lisp
(def (re im) (fft frame @N 1024 @backend accelerated))
(def (mag phase) (polar-fft re im))
```

For compatibility, `(fft x)` still also updates `__fft_re` and `__fft_im`.

### Spectral Effects

```lisp
(spectrum-delay spectrum @N 1024 @hops 4 @hop 256)
(spectrum-delay-mod spectrum delay @N 1024 @max-hops 32 @hop 256)
(phase-vocoder re im ratio @N 1024 @hop 256) ; returns (real imag)
(partition-ir irTensor @N 1024 @hop 256)     ; returns (irRe irIm)
(partitioned-spectral-mac xre xim irre irim @N 1024) ; returns (re im)
(partitioned-convolve input irTensor @N 1024 @hop 256 @gain 1.0)
```

These are intended for STFT effects, spectral delays/freezes, pitch shifting,
and IR/filter convolution. Use `buffer`, `hann`, `fft`, `ifft`, and
`overlap-add` for custom spectral processors.

### Windowing

```lisp
(hann 1024)                  ; periodic Hann tensor
(window @type hann @N 1024)  ; named window form
(buffer signal size)          ; ring buffer → [1, size] signalTensor
(buffer signal size hop)      ; with hop size
(overlap-add signalTensor hop) ; scatter-add into output signal
```

## Type System

DGenLisp has four value types:

| Type | Description |
|------|-------------|
| **float** | Compile-time constant (never hits the graph) |
| **signal** | Per-frame scalar (audio sample) |
| **tensor** | Static multi-dimensional array |
| **signalTensor** | Per-frame tensor (tensor that varies each audio frame) |

Floats are promoted automatically when combined with graph types. Signals and tensors produce signalTensors when combined.

## Manifest Format

```json
{
  "version": 1,
  "dylib": "patch.dylib",
  "sampleRate": 44100,
  "maxFrameCount": 4096,
  "totalMemorySlots": 256,
  "params": [{
    "name": "freq",
    "cellId": 84,
    "default": 440,
    "min": 20,
    "max": 20000,
    "unit": "Hz"
  }],
  "inputs": [{"channel": 0, "name": "signal"}],
  "outputs": [{"channel": 0, "name": "audio"}],
  "tensors": [{
    "name": "waves",
    "cellOffset": 100,
    "shape": [512, 32],
    "kind": "wavetable",
    "mutable": false,
    "sourceFile": "waves/factory.json"
  }],
  "tensorInitData": [{"offset": 100, "data": [0.5, ...]}]
}
```

- `cellId` values are **physical** memory offsets (after remapping), ready for direct indexing into the memory buffer
- `tensors` gives named metadata for tensor-backed assets and editable tensor slots
- `tensorInitData` entries must be written to the memory buffer before the first `process()` call
- `totalMemorySlots` is the required memory buffer size (in floats)

### Host Integration

The dylib exports:

```c
void process(
    float** inputs,      // input channel pointers
    float** outputs,     // output channel pointers
    int frameCount,      // number of frames to process
    void* memoryRead,    // memory buffer (read)
    void* memoryWrite    // memory buffer (write, usually same pointer)
);

void setParamValue(
    void* memory,        // memory buffer
    int cellId,          // physical cell ID from manifest
    float value          // new parameter value
);
```

## Examples

### Simple oscillator

```lisp
(param freq @default 440 @min 20 @max 20000 @unit Hz)
(out (sin (* (phasor freq) twopi)) 1 @name audio)
```

### Stereo

```lisp
(def phase (phasor 440))
(out (sin (* phase twopi)) 1 @name left)
(out (cos (* phase twopi)) 2 @name right)
```

### Allpass reverb with macros

```lisp
(defmacro ap (sig g d)
  (make-history h)
  (def ds (delay (read-history h) d))
  (def v (+ sig (* g ds)))
  (write-history h v)
  (- ds (* g v)))

(def input (in 1 @name signal))
(out (ap (ap input 0.7 11) 0.7 17) 1 @name audio)
```

### Filtered noise

```lisp
(param cutoff @default 1000 @min 100 @max 10000 @unit Hz)
(param q @default 2 @min 0.5 @max 20)
(out (biquad (noise) cutoff q 0 0) 1 @name audio)
```

### Compressor on input

```lisp
(def input (in 1 @name signal))
(out (compressor input @ratio 4 @threshold -20 @knee 6 @attack 0.01 @release 0.1) 1 @name audio)
```

### AM synthesis

```lisp
(param carrier @default 440 @min 20 @max 2000 @unit Hz)
(param modfreq @default 5 @min 0.1 @max 100 @unit Hz)
(param depth @default 0.5 @min 0 @max 1)

(def mod (+ 1 (* depth (sin (* (phasor modfreq) twopi)))))
(def osc (sin (* (phasor carrier) twopi)))
(out (* osc mod) 1 @name audio)
```
