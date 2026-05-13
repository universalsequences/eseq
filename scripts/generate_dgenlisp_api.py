#!/usr/bin/env python3
import argparse
import json
import os
import re
from collections import defaultdict
from pathlib import Path


DEFAULT_DGENLISP_ROOT = Path("/Users/alecresende/code/swift/dgen/Sources/DGenLisp")


CURATED_OPERATORS = {
    "+": {
        "category": "arithmetic",
        "summary": "Add two values. The parser rewrites n-ary forms into nested binary calls.",
        "signatures": ["(+ a b)", "(+ a b c ...)"],
        "arity": {"minimum": 2, "maximum": 2, "parser_rewrites_nary": True},
    },
    "-": {
        "category": "arithmetic",
        "summary": "Negate one value or subtract two values.",
        "signatures": ["(- x)", "(- a b)", "(- a b c ...)"],
        "arity": {"minimum": 1, "maximum": 2, "parser_rewrites_nary": True},
    },
    "*": {
        "category": "arithmetic",
        "summary": "Multiply two values. The parser rewrites n-ary forms into nested binary calls.",
        "signatures": ["(* a b)", "(* a b c ...)"],
        "arity": {"minimum": 2, "maximum": 2, "parser_rewrites_nary": True},
    },
    "/": {
        "category": "arithmetic",
        "summary": "Divide two values. The parser rewrites n-ary forms into nested binary calls.",
        "signatures": ["(/ a b)", "(/ a b c ...)"],
        "arity": {"minimum": 2, "maximum": 2, "parser_rewrites_nary": True},
    },
    "%": {
        "category": "arithmetic",
        "summary": "Modulo / remainder.",
        "signatures": ["(% a b)"],
        "arity": {"minimum": 2, "maximum": 2},
    },
    "sin": {"category": "math", "summary": "Sine.", "signatures": ["(sin x)"], "arity": {"minimum": 1, "maximum": 1}},
    "cos": {"category": "math", "summary": "Cosine.", "signatures": ["(cos x)"], "arity": {"minimum": 1, "maximum": 1}},
    "tan": {"category": "math", "summary": "Tangent.", "signatures": ["(tan x)"], "arity": {"minimum": 1, "maximum": 1}},
    "tanh": {"category": "math", "summary": "Hyperbolic tangent.", "signatures": ["(tanh x)"], "arity": {"minimum": 1, "maximum": 1}},
    "exp": {"category": "math", "summary": "Exponential.", "signatures": ["(exp x)"], "arity": {"minimum": 1, "maximum": 1}},
    "log": {"category": "math", "summary": "Natural logarithm.", "signatures": ["(log x)"], "arity": {"minimum": 1, "maximum": 1}},
    "sqrt": {"category": "math", "summary": "Square root.", "signatures": ["(sqrt x)"], "arity": {"minimum": 1, "maximum": 1}},
    "abs": {"category": "math", "summary": "Absolute value.", "signatures": ["(abs x)"], "arity": {"minimum": 1, "maximum": 1}},
    "sign": {"category": "math", "summary": "Sign function.", "signatures": ["(sign x)"], "arity": {"minimum": 1, "maximum": 1}},
    "floor": {"category": "math", "summary": "Floor.", "signatures": ["(floor x)"], "arity": {"minimum": 1, "maximum": 1}},
    "ceil": {"category": "math", "summary": "Ceiling.", "signatures": ["(ceil x)"], "arity": {"minimum": 1, "maximum": 1}},
    "round": {"category": "math", "summary": "Round.", "signatures": ["(round x)"], "arity": {"minimum": 1, "maximum": 1}},
    "relu": {"category": "math", "summary": "Rectified linear unit.", "signatures": ["(relu x)"], "arity": {"minimum": 1, "maximum": 1}},
    "sigmoid": {"category": "math", "summary": "Sigmoid.", "signatures": ["(sigmoid x)"], "arity": {"minimum": 1, "maximum": 1}},
    "log10": {"category": "math", "summary": "Base-10 logarithm.", "signatures": ["(log10 x)"], "arity": {"minimum": 1, "maximum": 1}},
    "pow": {"category": "math", "summary": "Exponentiation.", "signatures": ["(pow base exponent)"], "arity": {"minimum": 2, "maximum": 2}},
    "min": {
        "category": "math",
        "summary": "Minimum. The parser rewrites n-ary forms into nested binary calls.",
        "signatures": ["(min a b)", "(min a b c ...)"],
        "arity": {"minimum": 2, "maximum": 2, "parser_rewrites_nary": True},
    },
    "max": {
        "category": "math",
        "summary": "Maximum. The parser rewrites n-ary forms into nested binary calls.",
        "signatures": ["(max a b)", "(max a b c ...)"],
        "arity": {"minimum": 2, "maximum": 2, "parser_rewrites_nary": True},
    },
    "mse": {"category": "math", "summary": "Mean squared error.", "signatures": ["(mse prediction target)"], "arity": {"minimum": 2, "maximum": 2}},
    "atan2": {"category": "math", "summary": "Two-argument arctangent.", "signatures": ["(atan2 y x)"], "arity": {"minimum": 2, "maximum": 2}},
    "gt": {"category": "comparison", "summary": "Greater than.", "signatures": ["(gt a b)", "(> a b)"], "arity": {"minimum": 2, "maximum": 2}},
    "lt": {"category": "comparison", "summary": "Less than.", "signatures": ["(lt a b)", "(< a b)"], "arity": {"minimum": 2, "maximum": 2}},
    "gte": {"category": "comparison", "summary": "Greater than or equal.", "signatures": ["(gte a b)", "(>= a b)"], "arity": {"minimum": 2, "maximum": 2}},
    "lte": {"category": "comparison", "summary": "Less than or equal.", "signatures": ["(lte a b)", "(<= a b)"], "arity": {"minimum": 2, "maximum": 2}},
    "eq": {"category": "comparison", "summary": "Equality.", "signatures": ["(eq a b)", "(== a b)"], "arity": {"minimum": 2, "maximum": 2}},
    "phasor": {"category": "signal_generator", "summary": "Ramp oscillator from 0 to 1.", "signatures": ["(phasor freq)", "(phasor freq reset)"], "arity": {"minimum": 1, "maximum": 2}},
    "stateful-phasor": {"category": "signal_generator", "summary": "Forced stateful phasor variant.", "signatures": ["(stateful-phasor freq)"], "arity": {"minimum": 1, "maximum": 1}},
    "noise": {"category": "signal_generator", "summary": "White noise, or per-bin tensor noise with optional hop-rate updates.", "signatures": ["(noise)", "(noise @size 1024)", "(noise @size 1024 @hop 256)"], "arity": {"minimum": 0, "maximum": None}},
    "click": {"category": "signal_generator", "summary": "Impulse on the first frame.", "signatures": ["(click)"], "arity": {"minimum": 0, "maximum": 0}},
    "ramp2trig": {"category": "signal_generator", "summary": "Convert a ramp wrap into a trigger.", "signatures": ["(ramp2trig ramp)"], "arity": {"minimum": 1, "maximum": 1}},
    "accum": {"category": "stateful", "summary": "Accumulator with optional reset and bounds.", "signatures": ["(accum increment)", "(accum increment reset min max)"], "arity": {"minimum": 1, "maximum": 4}},
    "latch": {"category": "stateful", "summary": "Sample-and-hold.", "signatures": ["(latch value trigger)"], "arity": {"minimum": 2, "maximum": 2}},
    "mix": {"category": "stateful", "summary": "Linear interpolation.", "signatures": ["(mix a b t)"], "arity": {"minimum": 3, "maximum": 3}},
    "hop-hold": {"category": "stateful", "summary": "Hold a signal, tensor, or signalTensor for one hop interval.", "signatures": ["(hop-hold value hop)"], "arity": {"minimum": 2, "maximum": 2}},
    "biquad": {"category": "effect", "summary": "IIR biquad filter.", "signatures": ["(biquad signal cutoff q gain mode)", "(biquad signal @cutoff 1000 @q 0.707 @gain 0 @mode 0)"], "arity": {"minimum": 1, "maximum": 5}},
    "compressor": {"category": "effect", "summary": "Dynamics compressor with optional sidechain forms.", "signatures": ["(compressor signal ratio threshold knee attack release)", "(compressor signal ratio threshold knee attack release sidechain)", "(compressor signal ratio threshold knee attack release isSidechain sidechain)"], "arity": {"minimum": 1, "maximum": 8}},
    "delay": {"category": "effect", "summary": "Delay by a time in samples.", "signatures": ["(delay signal time_in_samples)"], "arity": {"minimum": 2, "maximum": 2}},
    "param": {"category": "io", "summary": "Host-visible scalar parameter.", "signatures": ["(param name @default value @min value @max value @unit string)"], "arity": {"minimum": 1, "maximum": None}},
    "in": {"category": "io", "summary": "Audio input channel.", "signatures": ["(in channel @name string)", "(in channel @name mod1 @modulator 1)"], "arity": {"minimum": 1, "maximum": None}},
    "out": {"category": "io", "summary": "Audio output channel.", "signatures": ["(out expr channel @name string)"], "arity": {"minimum": 2, "maximum": None}},
    "tensor": {"category": "tensor_creation", "summary": "Zero-filled tensor alias, or inline data tensor with @shape/@data.", "signatures": ["(tensor rows cols)", "(tensor [d1,d2,...])", "(tensor @shape [4] @data [1 0.5 0.25 0])"], "arity": {"minimum": 0, "maximum": None}},
    "zeros": {"category": "tensor_creation", "summary": "Zero-filled tensor.", "signatures": ["(zeros [d1,d2,...])", "(zeros d1 d2 ...)"], "arity": {"minimum": 1, "maximum": None}},
    "ones": {"category": "tensor_creation", "summary": "All-ones tensor.", "signatures": ["(ones [d1,d2,...])", "(ones d1 d2 ...)"], "arity": {"minimum": 1, "maximum": None}},
    "full": {"category": "tensor_creation", "summary": "Constant-filled tensor.", "signatures": ["(full [d1,d2,...] value)", "(full d1 d2 ... value)"], "arity": {"minimum": 2, "maximum": None}},
    "randn": {"category": "tensor_creation", "summary": "Random normal tensor.", "signatures": ["(randn [d1,d2,...])", "(randn d1 d2 ...)"], "arity": {"minimum": 1, "maximum": None}},
    "tensor-param": {"category": "tensor_creation", "summary": "Host-visible tensor parameter.", "signatures": ["(tensor-param [d1,d2,...])"], "arity": {"minimum": 1, "maximum": None}},
    "audio-tensor": {"category": "tensor_creation", "summary": "Load a WAV file into static tensor manifest data.", "signatures": ["(audio-tensor @file \"irs/hall.wav\")", "(audio-tensor @file \"stereo.wav\" @channel 0)", "(audio-tensor @file \"x.wav\" @mono true @normalize peak)"], "arity": {"minimum": 0, "maximum": None}},
    "ir": {"category": "tensor_creation", "summary": "Load a WAV file as an impulse-response tensor.", "signatures": ["(ir @file \"irs/room.wav\")"], "arity": {"minimum": 0, "maximum": None}},
    "matmul": {"category": "tensor_op", "summary": "Matrix multiplication.", "signatures": ["(matmul a b)"], "arity": {"minimum": 2, "maximum": 2}},
    "conv1d": {"category": "tensor_op", "summary": "1D convolution.", "signatures": ["(conv1d input kernel)"], "arity": {"minimum": 2, "maximum": 2}},
    "peek": {"category": "tensor_op", "summary": "Read a scalar from a tensor.", "signatures": ["(peek tensor index)", "(peek tensor index channel)"], "arity": {"minimum": 2, "maximum": 3}},
    "peek-row": {"category": "tensor_op", "summary": "Read a tensor row as a signalTensor.", "signatures": ["(peek-row tensor rowIndex)"], "arity": {"minimum": 2, "maximum": 2}},
    "sample": {"category": "tensor_op", "summary": "Interpolated row read from a tensor.", "signatures": ["(sample tensor index)"], "arity": {"minimum": 2, "maximum": 2}},
    "to-signal": {"category": "tensor_op", "summary": "Convert a 1D tensor into a signal playback source.", "signatures": ["(to-signal tensor)", "(to-signal tensor @max-frames 4096)"], "arity": {"minimum": 1, "maximum": None}},
    "reshape": {"category": "tensor_shape", "summary": "Reshape tensor dimensions.", "signatures": ["(reshape tensor @shape [d1,d2,...])"], "arity": {"minimum": 1, "maximum": None}},
    "transpose": {"category": "tensor_shape", "summary": "Transpose / permute tensor axes.", "signatures": ["(transpose tensor)", "(transpose tensor @axes [1,0])"], "arity": {"minimum": 1, "maximum": None}},
    "shrink": {"category": "tensor_shape", "summary": "Slice a tensor.", "signatures": ["(shrink tensor @ranges [0:2,1:3])"], "arity": {"minimum": 1, "maximum": None}},
    "pad": {"category": "tensor_shape", "summary": "Pad a tensor.", "signatures": ["(pad tensor @padding [1:1,0:0])"], "arity": {"minimum": 1, "maximum": None}},
    "expand": {"category": "tensor_shape", "summary": "Broadcast expand a tensor.", "signatures": ["(expand tensor @shape [4,3])"], "arity": {"minimum": 1, "maximum": None}},
    "repeat": {"category": "tensor_shape", "summary": "Tile / repeat a tensor.", "signatures": ["(repeat tensor @repeats [2,3])"], "arity": {"minimum": 1, "maximum": None}},
    "conv2d": {"category": "tensor_shape", "summary": "2D convolution.", "signatures": ["(conv2d input kernel)"], "arity": {"minimum": 2, "maximum": 2}},
    "windows": {"category": "tensor_shape", "summary": "Extract sliding windows / im2col-style tensor view.", "signatures": ["(windows tensor @shape [3 3])"], "arity": {"minimum": 1, "maximum": None}},
    "sum": {"category": "reduction", "summary": "Sum reduction.", "signatures": ["(sum tensor)", "(sum tensor @axis 0)"], "arity": {"minimum": 1, "maximum": None}},
    "mean": {"category": "reduction", "summary": "Mean reduction.", "signatures": ["(mean tensor)", "(mean tensor @axis 0)"], "arity": {"minimum": 1, "maximum": None}},
    "max-axis": {"category": "reduction", "summary": "Maximum along a specific axis.", "signatures": ["(max-axis tensor @axis 0)"], "arity": {"minimum": 1, "maximum": None}},
    "sum-axis": {"category": "reduction", "summary": "Explicit axis sum.", "signatures": ["(sum-axis tensor @axis 0)"], "arity": {"minimum": 1, "maximum": None}},
    "mean-axis": {"category": "reduction", "summary": "Explicit axis mean.", "signatures": ["(mean-axis tensor @axis 0)"], "arity": {"minimum": 1, "maximum": None}},
    "softmax": {"category": "reduction", "summary": "Softmax along an axis.", "signatures": ["(softmax tensor @axis -1)"], "arity": {"minimum": 1, "maximum": None}},
    "fft": {"category": "fft", "summary": "FFT returning `(real imag)`; also updates legacy `__fft_re` and `__fft_im`.", "signatures": ["(fft input)", "(fft input N)", "(fft input @N 1024 @backend accelerated)"], "arity": {"minimum": 1, "maximum": None}},
    "ifft": {"category": "fft", "summary": "Inverse FFT from real and imaginary parts.", "signatures": ["(ifft real imag)", "(ifft real imag N)", "(ifft real imag @N 1024 @backend accelerated)"], "arity": {"minimum": 2, "maximum": None}},
    "polar-fft": {"category": "fft", "summary": "Convert rectangular complex spectrum to `(magnitude phase)`.", "signatures": ["(polar-fft re im)"], "arity": {"minimum": 2, "maximum": 2}},
    "rect-fft": {"category": "fft", "summary": "Convert polar spectrum to `(real imag)`.", "signatures": ["(rect-fft mag phase)"], "arity": {"minimum": 2, "maximum": 2}},
    "complex-mul": {"category": "fft", "summary": "Complex multiply, returning `(real imag)`.", "signatures": ["(complex-mul ar ai br bi)"], "arity": {"minimum": 4, "maximum": 4}},
    "complex-conj": {"category": "fft", "summary": "Complex conjugate, returning `(real -imag)`.", "signatures": ["(complex-conj re im)"], "arity": {"minimum": 2, "maximum": 2}},
    "spectrum-delay": {"category": "spectral_effect", "summary": "Delay a spectrum by a fixed number of hops.", "signatures": ["(spectrum-delay spectrum @N 1024 @hops 4 @hop 256)"], "arity": {"minimum": 1, "maximum": None}},
    "spectrum-delay-mod": {"category": "spectral_effect", "summary": "Delay a spectrum by a signal-controlled number of hops.", "signatures": ["(spectrum-delay-mod spectrum delay @N 1024 @max-hops 32 @hop 256)"], "arity": {"minimum": 2, "maximum": None}},
    "phase-vocoder": {"category": "spectral_effect", "summary": "Phase-vocoder bin remap/pitch transform, returning `(real imag)`.", "signatures": ["(phase-vocoder re im ratio @N 1024 @hop 256)"], "arity": {"minimum": 3, "maximum": None}},
    "partition-ir": {"category": "spectral_effect", "summary": "Prepartition an impulse response, returning `(irRe irIm)`.", "signatures": ["(partition-ir irTensor @N 1024 @hop 256)"], "arity": {"minimum": 1, "maximum": None}},
    "partitioned-spectral-mac": {"category": "spectral_effect", "summary": "Multiply-accumulate an input spectrum against partitioned IR spectra, returning `(real imag)`.", "signatures": ["(partitioned-spectral-mac xre xim irre irim @N 1024)"], "arity": {"minimum": 4, "maximum": None}},
    "partitioned-convolve": {"category": "spectral_effect", "summary": "High-level partitioned convolution from signal and IR tensor.", "signatures": ["(partitioned-convolve input irTensor @N 1024 @hop 256)", "(partitioned-convolve input irTensor @N 1024 @hop 256 @gain 1.0)"], "arity": {"minimum": 2, "maximum": None}},
    "buffer": {"category": "windowing", "summary": "Ring buffer into a signalTensor.", "signatures": ["(buffer signal size)", "(buffer signal size hop)"], "arity": {"minimum": 2, "maximum": 3}},
    "hann": {"category": "windowing", "summary": "Periodic Hann window tensor.", "signatures": ["(hann 1024)"], "arity": {"minimum": 1, "maximum": 1}},
    "window": {"category": "windowing", "summary": "Named window tensor. Currently supports Hann.", "signatures": ["(window @type hann @N 1024)"], "arity": {"minimum": 0, "maximum": None}},
    "overlap-add": {"category": "windowing", "summary": "Scatter-add a signalTensor into an output signal.", "signatures": ["(overlap-add signalTensor hop)"], "arity": {"minimum": 2, "maximum": 2}},
    "scale": {"category": "utility", "summary": "Linear rescale.", "signatures": ["(scale sig inMin inMax outMin outMax)"], "arity": {"minimum": 5, "maximum": 5}},
    "triangle": {"category": "utility", "summary": "Convert a 0..1 phase to a -1..1 triangle.", "signatures": ["(triangle phase)"], "arity": {"minimum": 1, "maximum": 1}},
    "wrap": {"category": "utility", "summary": "Wrap into a range.", "signatures": ["(wrap sig min max)"], "arity": {"minimum": 3, "maximum": 3}},
    "clip": {"category": "utility", "summary": "Clamp into a range.", "signatures": ["(clip sig min max)"], "arity": {"minimum": 3, "maximum": 3}},
    "gswitch": {"category": "conditional", "summary": "Conditional branch.", "signatures": ["(gswitch condition true_value false_value)"], "arity": {"minimum": 3, "maximum": 3}},
    "selector": {"category": "conditional", "summary": "1-based selector over options; mode <= 0 yields 0.", "signatures": ["(selector mode option1 option2 ...)"], "arity": {"minimum": 2, "maximum": None}},
}


SPECIAL_FORMS = [
    {
        "name": "def",
        "summary": "Bind a symbol to the last evaluated body expression.",
        "signatures": ["(def name expr)", "(def name expr1 expr2 ...)", "(def (name1 name2) tupleExpr)"],
    },
    {
        "name": "defmacro",
        "summary": "Define a macro with hygienic local `def` and `make-history` scoping.",
        "signatures": ["(defmacro name (params...) body...)"],
    },
    {
        "name": "make-history",
        "summary": "Create a history cell for feedback.",
        "signatures": ["(make-history name)"],
    },
    {
        "name": "read-history",
        "summary": "Read the previous frame from a history cell.",
        "signatures": ["(read-history name)"],
    },
    {
        "name": "write-history",
        "summary": "Write the current frame to a history cell and return the written signal.",
        "signatures": ["(write-history name expr)"],
    },
    {
        "name": "make-tensor-history",
        "summary": "Create a tensor history cell for recurrent tensor DSP.",
        "signatures": ["(make-tensor-history name @shape [H W])", "(make-tensor-history name @shape [H W] @data [...])"],
    },
    {
        "name": "read-tensor-history",
        "summary": "Read the previous tensor value from a tensor history cell.",
        "signatures": ["(read-tensor-history name)"],
    },
    {
        "name": "write-tensor-history",
        "summary": "Write the current tensor value and return it.",
        "signatures": ["(write-tensor-history name expr)"],
    },
    {
        "name": "mod",
        "summary": "Resolve the lowered modulated value for a parameter declared with `@mod true`.",
        "signatures": ["(mod paramName)"],
    },
]


CONSTANTS = [
    {"name": "pi", "value": "pi", "summary": "Pi."},
    {"name": "twopi", "value": "2*pi", "summary": "Two pi."},
    {"name": "tau", "value": "2*pi", "summary": "Alias for twopi."},
    {"name": "e", "value": "Euler's number", "summary": "Euler's constant."},
    {"name": "true", "value": 1.0, "summary": "Boolean true as float."},
    {"name": "false", "value": 0.0, "summary": "Boolean false as float."},
]


CLI_OPTIONS = [
    {"flag": "-o", "long_flag": "--output", "value": "<dir>", "default": ".", "summary": "Output directory."},
    {"flag": None, "long_flag": "--name", "value": "<name>", "default": "patch", "summary": "Output name without extension."},
    {"flag": None, "long_flag": "--sample-rate", "value": "<rate>", "default": 44100, "summary": "Sample rate in Hz."},
    {"flag": None, "long_flag": "--max-frames", "value": "<count>", "default": 4096, "summary": "Maximum frame count."},
    {"flag": None, "long_flag": "--voices", "value": "<count>", "default": 1, "summary": "Voice count for polyphony."},
    {"flag": None, "long_flag": "--debug", "value": None, "default": False, "summary": "Enable debug output."},
    {"flag": "-", "long_flag": None, "value": None, "default": True, "summary": "Read source from stdin."},
]


GLOBAL_ATTRIBUTES = {
    "@default": "Default value for params and generated modulation helpers.",
    "@min": "Minimum parameter value.",
    "@max": "Maximum parameter value.",
    "@unit": "Host-visible unit label.",
    "@name": "Host-visible input/output or modulator name.",
    "@modulator": "Marks an input as a modulation source slot.",
    "@mod": "Marks a parameter as modulatable.",
    "@mod-mode": "Modulation mode: additive, multiplicative, or semitone.",
    "@mod-depth-min": "Lower bound for generated modulation depth control.",
    "@mod-depth-max": "Upper bound for generated modulation depth control.",
    "@hidden": "Hide a generated or internal parameter from normal host presentation.",
    "@generated": "Tags generated helper parameters.",
    "@generated-for": "Associates a generated helper parameter with a user parameter.",
    "@mod-source-param": "Generated modulation source parameter name.",
    "@mod-depth-param": "Generated modulation depth parameter name.",
    "@mod-resolved-symbol": "Generated resolved modulation symbol name.",
    "@cutoff": "Biquad cutoff attribute form.",
    "@q": "Biquad resonance / Q attribute form.",
    "@gain": "Biquad gain attribute form.",
    "@mode": "Biquad filter mode attribute form.",
    "@ratio": "Compressor ratio.",
    "@threshold": "Compressor threshold.",
    "@knee": "Compressor knee.",
    "@attack": "Compressor attack.",
    "@release": "Compressor release.",
    "@sidechain": "Compressor sidechain signal binding.",
    "@max-frames": "Playback frame budget for `to-signal`.",
    "@shape": "Target shape for reshape / expand.",
    "@axes": "Axis order for transpose.",
    "@ranges": "Slice ranges for shrink.",
    "@padding": "Per-axis padding for pad.",
    "@repeats": "Per-axis repeat counts.",
    "@axis": "Axis argument for reductions and softmax.",
    "@N": "FFT/window size for spectral operators.",
    "@backend": "FFT backend: tensor or accelerated.",
    "@size": "Tensor size for tensor noise.",
    "@hop": "Hop size for STFT and hop-rate operators.",
    "@hops": "Fixed delay in hops for spectrum-delay.",
    "@max-hops": "Maximum modulated delay in hops for spectrum-delay-mod.",
    "@type": "Window type for window operator.",
    "@file": "Audio or tensor asset path.",
    "@mono": "Load audio as mono.",
    "@channel": "Load one channel from an audio asset.",
    "@start": "Audio load start time in seconds.",
    "@end": "Audio load end time in seconds.",
    "@normalize": "Audio load normalization mode, e.g. peak.",
    "@data": "Inline tensor or tensor-history data.",
    "@gain": "Gain for high-level spectral convolution.",
}


MANIFEST_SCHEMA = {
    "version": {"type": "int", "required": True},
    "dylib": {"type": "string", "required": True},
    "cSourcePath": {"type": "string", "required": True},
    "sampleRate": {"type": "float", "required": True},
    "maxFrameCount": {"type": "int", "required": True},
    "voiceCount": {"type": "int", "required": True},
    "voiceCellId": {"type": "int|null", "required": False},
    "totalMemorySlots": {"type": "int", "required": True},
    "params": {"type": "ManifestParam[]", "required": True},
    "inputs": {"type": "ManifestInput[]", "required": True},
    "outputs": {"type": "ManifestOutput[]", "required": True},
    "modulators": {"type": "ManifestModulator[]", "required": True},
    "modDestinations": {"type": "ManifestModDestination[]", "required": True},
    "tensorInitData": {"type": "ManifestTensorInit[]", "required": True},
}


MANIFEST_TYPES = {
    "ManifestParam": {
        "name": {"type": "string"},
        "cellId": {"type": "int"},
        "default": {"type": "float"},
        "min": {"type": "float|null"},
        "max": {"type": "float|null"},
        "unit": {"type": "string|null"},
        "hidden": {"type": "bool|null"},
    },
    "ManifestInput": {
        "channel": {"type": "int"},
        "name": {"type": "string|null"},
    },
    "ManifestOutput": {
        "channel": {"type": "int"},
        "name": {"type": "string|null"},
    },
    "ManifestModulator": {
        "slot": {"type": "int"},
        "inputChannel": {"type": "int"},
        "name": {"type": "string|null"},
    },
    "ManifestModDestination": {
        "name": {"type": "string"},
        "paramCellId": {"type": "int"},
        "mode": {"type": "string"},
        "sourceCellId": {"type": "int"},
        "depthCellId": {"type": "int"},
        "min": {"type": "float"},
        "max": {"type": "float"},
        "unit": {"type": "string|null"},
        "depthMin": {"type": "float|null"},
        "depthMax": {"type": "float|null"},
    },
    "ManifestTensorInit": {
        "offset": {"type": "int"},
        "data": {"type": "float[]"},
    },
}


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def extract_function_attribute_usage(evaluator_source: str):
    fn_pattern = re.compile(
        r'private func ([A-Za-z0-9_]+)\([^)]*attributes: \[\(name: String, value: String\)\][^)]*\) throws -> EvalResult \{',
        re.MULTILINE,
    )
    matches = list(fn_pattern.finditer(evaluator_source))
    attrs_by_fn = {}
    for idx, match in enumerate(matches):
        fn_name = match.group(1)
        start = match.end()
        end = matches[idx + 1].start() if idx + 1 < len(matches) else len(evaluator_source)
        body = evaluator_source[start:end]
        attrs = sorted(set(re.findall(r'attrValue\(attributes,\s*"(@[^"]+)"\)', body)))
        attrs_by_fn[fn_name] = attrs
    return attrs_by_fn


def extract_operator_cases(evaluator_source: str):
    start = evaluator_source.index("switch op {")
    end = evaluator_source.index("default:\n            throw LispError.unknownOperator(opName)")
    block = evaluator_source[start:end]
    operator_map = {}
    lines = block.splitlines()
    idx = 0
    while idx < len(lines):
        stripped = lines[idx].strip()
        if not stripped.startswith("case "):
            idx += 1
            continue
        case_part, _, _ = stripped.partition(":")
        raw_names = case_part[len("case "):]
        names = [part.strip().strip('"') for part in raw_names.split(",")]
        body_lines = []
        idx += 1
        while idx < len(lines):
            next_line = lines[idx].strip()
            if next_line.startswith("case ") or next_line.startswith("default:"):
                break
            body_lines.append(next_line)
            idx += 1
        body = "\n".join(body_lines)
        impl = None
        m = re.search(r"return try ([A-Za-z0-9_]+)\(", body)
        if m is not None:
            impl = m.group(1)
        else:
            m = re.search(r"return \.([A-Za-z0-9_]+)\(", body)
            if m is not None:
                impl = m.group(1)
        if impl is None:
            continue
        operator_map[names[0]] = {"aliases": names[1:], "implementation": impl}
    return operator_map


def collect_examples(repo_root: Path):
    examples = []
    for kind in ("instruments", "effects"):
        base = repo_root / kind
        if not base.exists():
            continue
        for path in sorted(base.glob("*.lisp")):
            text = read(path)
            params = re.findall(r"\(param\s+([^\s\)]+)", text)
            outputs = len(re.findall(r"\(out\s", text))
            modulators = len(re.findall(r"@modulator\s+\d+", text))
            preview_lines = []
            for line in text.splitlines():
                stripped = line.strip()
                if not stripped or stripped.startswith(";") or stripped.startswith("#"):
                    continue
                preview_lines.append(line.rstrip())
                if len(preview_lines) == 6:
                    break
            examples.append(
                {
                    "name": path.stem,
                    "kind": kind[:-1],
                    "path": str(path.relative_to(repo_root)),
                    "params": sorted(set(params)),
                    "output_count": outputs,
                    "modulator_count": modulators,
                    "preview": "\n".join(preview_lines),
                }
            )
    return examples


def build_attributes_index(operator_map, attrs_by_fn):
    usage = defaultdict(set)
    for name, meta in operator_map.items():
        fn_name = meta["implementation"]
        for attr in attrs_by_fn.get(fn_name, []):
            usage[attr].add(name)
    all_attrs = []
    for attr in sorted(set(GLOBAL_ATTRIBUTES) | set(usage)):
        all_attrs.append(
            {
                "name": attr,
                "summary": GLOBAL_ATTRIBUTES.get(attr, "Attribute observed in evaluator source."),
                "used_by": sorted(usage.get(attr, [])),
            }
        )
    return all_attrs


def build_operators(operator_map, attrs_by_fn):
    operators = []
    for name in sorted(operator_map):
        base = operator_map[name]
        curated = CURATED_OPERATORS.get(name, {})
        operators.append(
            {
                "name": name,
                "aliases": base["aliases"],
                "category": curated.get("category", "uncategorized"),
                "summary": curated.get("summary", "Operator implemented in DGenLisp evaluator."),
                "signatures": curated.get("signatures", []),
                "arity": curated.get("arity", {"minimum": None, "maximum": None}),
                "attributes": attrs_by_fn.get(base["implementation"], []),
                "implementation": {
                    "function": base["implementation"],
                    "source_file": "LispEvaluator.swift",
                },
            }
        )
    return operators


def main():
    parser = argparse.ArgumentParser(description="Generate structured DGenLisp API data for sequencer.")
    parser.add_argument("--dgenlisp-root", default=str(DEFAULT_DGENLISP_ROOT))
    parser.add_argument("--repo-root", default=str(Path(__file__).resolve().parents[1]))
    parser.add_argument("--output", default=None)
    args = parser.parse_args()

    dgen_root = Path(os.path.expanduser(args.dgenlisp_root)).resolve()
    repo_root = Path(args.repo_root).resolve()
    output = Path(args.output).resolve() if args.output else repo_root / "docs" / "dgenlisp-api.json"

    evaluator_source = read(dgen_root / "LispEvaluator.swift")
    operator_map = extract_operator_cases(evaluator_source)
    attrs_by_fn = extract_function_attribute_usage(evaluator_source)
    examples = collect_examples(repo_root)

    data = {
        "schema_version": 1,
        "generated_from": {
            "dgenlisp_root": str(dgen_root),
            "source_files": [
                "README.md",
                "main.swift",
                "Manifest.swift",
                "ModulationLowering.swift",
                "LispEvaluator.swift",
            ],
        },
        "language": {
            "comments": [{"prefix": ";"}, {"prefix": "#"}],
            "constants": CONSTANTS,
            "special_forms": SPECIAL_FORMS,
            "operators": build_operators(operator_map, attrs_by_fn),
            "attributes": build_attributes_index(operator_map, attrs_by_fn),
            "types": [
                {"name": "float", "summary": "Compile-time scalar constant."},
                {"name": "signal", "summary": "Per-frame scalar signal."},
                {"name": "tensor", "summary": "Static multi-dimensional array."},
                {"name": "signalTensor", "summary": "Per-frame tensor value."},
            ],
            "modulation": {
                "modes": ["additive", "multiplicative", "semitone"],
                "special_form": "(mod paramName)",
                "generated_helpers": [
                    "__mod__<param>__source",
                    "__mod__<param>__depth",
                    "__mod__<param>__resolved",
                ],
                "required_param_attributes": ["@mod true", "@mod-mode", "@min", "@max"],
                "notes": [
                    "Modulatable params require at least one input marked with @modulator.",
                    "Generated modulation source and depth params are hidden host parameters.",
                ],
            },
            "compiler_cli": {
                "command": "dgenlisp compile [<file.lisp>] [options]",
                "options": CLI_OPTIONS,
                "outputs": [
                    "<name>.dylib",
                    "<name>.json",
                ],
            },
            "manifest": {
                "schema": MANIFEST_SCHEMA,
                "types": MANIFEST_TYPES,
            },
            "examples": examples,
        },
    }

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
