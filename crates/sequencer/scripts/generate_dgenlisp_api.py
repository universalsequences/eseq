#!/usr/bin/env python3
import argparse
import json
import os
import re
from collections import defaultdict
from pathlib import Path


DEFAULT_DGENLISP_ROOT = Path("/Users/alecresende/code/swift/dgen/Sources/DGenLisp")
DEFAULT_OPERATOR_MANIFEST = Path("tools/dgenlisp-operators.json")


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
    "param": {
        "category": "io",
        "summary": "Host-visible scalar parameter.",
        "signatures": [
            "(param name @default value @min value @max value @unit string)",
            "(param name @group groupName @env envName @role attack|decay|sustain|release)",
        ],
        "arity": {"minimum": 1, "maximum": None},
    },
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
    "triangle": {"category": "utility", "summary": "Convert a 0..1 phase to a triangle wave with optional duty.", "signatures": ["(triangle phase)", "(triangle phase duty)"], "arity": {"minimum": 1, "maximum": 2}},
    "wrap": {"category": "utility", "summary": "Wrap into a range.", "signatures": ["(wrap sig min max)"], "arity": {"minimum": 3, "maximum": 3}},
    "clip": {"category": "utility", "summary": "Clamp into a range.", "signatures": ["(clip sig min max)"], "arity": {"minimum": 3, "maximum": 3}},
    "gswitch": {"category": "conditional", "summary": "Conditional branch.", "signatures": ["(gswitch condition true_value false_value)"], "arity": {"minimum": 3, "maximum": 3}},
    "selector": {"category": "conditional", "summary": "1-based selector over options; mode <= 0 yields 0.", "signatures": ["(selector mode option1 option2 ...)"], "arity": {"minimum": 2, "maximum": None}},
    "atan": {"category": "math", "summary": "Arctangent.", "signatures": ["(atan x)"], "arity": {"minimum": 1, "maximum": 1}},
    "wavetable": {"category": "tensor_creation", "summary": "Load a static wavetable tensor from JSON data, or create zero-filled wavetable data from a shape.", "signatures": ["(wavetable @shape [512 32] @file \"waves/factory.json\")", "(wavetable [512 32])"], "arity": {"minimum": 0, "maximum": None}},
    "wavetable-param": {"category": "tensor_creation", "summary": "Host-editable wavetable tensor initialized from JSON data or zeros.", "signatures": ["(wavetable-param @shape [512 32] @default-file \"waves/init.json\")", "(wavetable-param [512 32])"], "arity": {"minimum": 0, "maximum": None}},
    "__modulated-param": {"category": "internal", "summary": "Internal lowered modulation combiner. User code should use `(mod paramName)` instead.", "signatures": ["(__modulated-param base active modulator depth ... @mode additive @min 0 @max 1)"], "arity": {"minimum": 2, "maximum": None}},
}


CURATED_OPERATOR_UPDATES = {
    "stateful-phasor": {
        "summary": "Forced stateful phasor variant with optional reset.",
        "signatures": ["(stateful-phasor freq)", "(stateful-phasor freq reset)"],
        "arity": {"minimum": 1, "maximum": 2},
    },
    "triangle": {
        "summary": "Convert a 0..1 phase to a triangle wave with optional duty.",
        "signatures": ["(triangle phase)", "(triangle phase duty)"],
        "arity": {"minimum": 1, "maximum": 2},
    },
    "wrap": {
        "summary": "Wrap into a range. Defaults to 0..1 when bounds are omitted.",
        "signatures": ["(wrap sig)", "(wrap sig min max)"],
        "arity": {"minimum": 1, "maximum": 3},
    },
}

for _name, _updates in CURATED_OPERATOR_UPDATES.items():
    CURATED_OPERATORS.setdefault(_name, {}).update(_updates)


ATTRIBUTE_SPECS = {
    "@N": {"type": "int", "summary": "FFT/window size for spectral operators.", "aliases": ["@n"]},
    "@n": {"type": "int", "summary": "Lowercase alias for @N.", "aliases": ["@N"]},
    "@attack": {"type": "signal|float", "summary": "Compressor attack time."},
    "@axes": {"type": "int[]", "summary": "Axis order for transpose."},
    "@axis": {"type": "int", "summary": "Axis argument for reductions and softmax."},
    "@backend": {"type": "enum", "values": ["tensor", "accelerated"], "summary": "FFT backend."},
    "@channel": {"type": "int", "summary": "Audio file channel to load. Channel numbering is zero-based."},
    "@cutoff": {"type": "signal|float", "summary": "Biquad cutoff in Hz."},
    "@data": {"type": "float[]", "summary": "Inline tensor or tensor-history data."},
    "@default": {"type": "float", "summary": "Default value for scalar params."},
    "@default-file": {"type": "path", "summary": "JSON file used to initialize a mutable tensor or wavetable parameter."},
    "@end": {"type": "float", "summary": "Audio load end time in seconds."},
    "@env": {"type": "symbol", "summary": "UI fallback envelope name for an envelope-role param."},
    "@file": {"type": "path", "summary": "Audio, tensor, or wavetable asset path."},
    "@gain": {"type": "signal|float", "summary": "Biquad gain or partitioned convolution output gain, depending on operator."},
    "@generated": {"type": "string", "summary": "Tags generated helper parameters."},
    "@generated-for": {"type": "symbol", "summary": "Associates a generated helper parameter with a user parameter."},
    "@group": {"type": "symbol", "summary": "UI fallback group name for a host-visible param."},
    "@hidden": {"type": "bool", "summary": "Hide a generated or internal parameter from normal host presentation."},
    "@hop": {"type": "int", "summary": "Hop size for STFT and hop-rate operators.", "aliases": ["@hopSize"]},
    "@hopSize": {"type": "int", "summary": "Camel-case alias for @hop.", "aliases": ["@hop"]},
    "@hops": {"type": "int", "summary": "Fixed delay in hops for spectrum-delay."},
    "@knee": {"type": "signal|float", "summary": "Compressor knee in dB."},
    "@max": {"type": "float", "summary": "Maximum parameter value."},
    "@max-frames": {"type": "int", "summary": "Playback frame budget for to-signal."},
    "@max-hops": {"type": "int", "summary": "Maximum modulated delay in hops for spectrum-delay-mod.", "aliases": ["@maxHops"]},
    "@maxHops": {"type": "int", "summary": "Camel-case alias for @max-hops.", "aliases": ["@max-hops"]},
    "@min": {"type": "float", "summary": "Minimum parameter value."},
    "@mode": {"type": "enum|signal|float", "summary": "Operator mode selector. Meaning is operator-specific."},
    "@mod": {"type": "bool", "summary": "Marks a parameter as modulatable."},
    "@mod-active-param": {"type": "symbol", "summary": "Generated modulation active parameter name."},
    "@mod-depth-max": {"type": "float", "summary": "Upper bound for generated modulation depth control."},
    "@mod-depth-min": {"type": "float", "summary": "Lower bound for generated modulation depth control."},
    "@mod-mode": {"type": "enum", "values": ["additive", "multiplicative", "semitone"], "summary": "Modulation mode for a modulatable param."},
    "@mod-resolved-symbol": {"type": "symbol", "summary": "Generated resolved modulation symbol name."},
    "@modulator": {"type": "int", "summary": "Marks an input as a modulation source slot."},
    "@modulator-slot": {"type": "int", "summary": "Generated modulation depth parameter slot."},
    "@mono": {"type": "bool", "summary": "Load audio as mono when no explicit channel is selected."},
    "@name": {"type": "string", "summary": "Host-visible parameter, input, output, or tensor name."},
    "@normalize": {"type": "enum", "values": ["peak"], "summary": "Audio load normalization mode."},
    "@padding": {"type": "range[]", "summary": "Per-axis padding for pad, e.g. [1:1,0:0]."},
    "@q": {"type": "signal|float", "summary": "Biquad resonance / Q."},
    "@ranges": {"type": "range[]", "summary": "Slice ranges for shrink, e.g. [0:2,1:3]."},
    "@ratio": {"type": "signal|float", "summary": "Compressor ratio."},
    "@release": {"type": "signal|float", "summary": "Compressor release time."},
    "@repeats": {"type": "int[]", "summary": "Per-axis repeat counts."},
    "@role": {"type": "enum", "values": ["attack", "decay", "sustain", "release"], "summary": "Envelope role for a param that declares @env."},
    "@shape": {"type": "int[]", "summary": "Tensor shape."},
    "@sidechain": {"type": "symbol|expr", "summary": "Compressor sidechain signal."},
    "@size": {"type": "int", "summary": "Tensor size for tensor noise."},
    "@start": {"type": "float", "summary": "Audio load start time in seconds."},
    "@threshold": {"type": "signal|float", "summary": "Compressor threshold in dB."},
    "@type": {"type": "enum", "values": ["hann"], "summary": "Window type."},
    "@unit": {"type": "string", "summary": "Host-visible unit label."},
}


CURATED_OPERATOR_ATTRIBUTES = {
    "__modulated-param": ["@mode", "@min", "@max"],
    "audio-tensor": ["@file", "@channel", "@mono", "@start", "@end", "@normalize", "@name"],
    "biquad": ["@cutoff", "@q", "@gain", "@mode"],
    "compressor": ["@ratio", "@threshold", "@knee", "@attack", "@release", "@sidechain"],
    "fft": ["@N", "@n", "@backend"],
    "hann": ["@N", "@n"],
    "ifft": ["@N", "@n", "@backend"],
    "in": ["@name", "@modulator"],
    "ir": ["@file", "@channel", "@mono", "@start", "@end", "@normalize", "@name"],
    "make-tensor-history": ["@shape", "@data"],
    "noise": ["@size", "@hop", "@hopSize"],
    "out": ["@name"],
    "param": [
        "@default",
        "@min",
        "@max",
        "@unit",
        "@group",
        "@env",
        "@role",
        "@hidden",
        "@mod",
        "@mod-mode",
        "@mod-depth-min",
        "@mod-depth-max",
        "@mod-active-param",
        "@mod-resolved-symbol",
        "@generated",
        "@generated-for",
        "@modulator-slot",
    ],
    "partition-ir": ["@N", "@n", "@hop", "@hopSize"],
    "partitioned-convolve": ["@N", "@n", "@hop", "@hopSize", "@gain"],
    "partitioned-spectral-mac": ["@N", "@n"],
    "phase-vocoder": ["@N", "@n", "@hop", "@hopSize"],
    "reshape": ["@shape"],
    "shrink": ["@ranges"],
    "pad": ["@padding"],
    "expand": ["@shape"],
    "repeat": ["@repeats"],
    "sum": ["@axis"],
    "mean": ["@axis"],
    "max-axis": ["@axis"],
    "sum-axis": ["@axis"],
    "mean-axis": ["@axis"],
    "softmax": ["@axis"],
    "spectrum-delay": ["@N", "@n", "@hops", "@hop", "@hopSize"],
    "spectrum-delay-mod": ["@N", "@n", "@max-hops", "@maxHops", "@hop", "@hopSize"],
    "tensor": ["@shape", "@data", "@file", "@name"],
    "tensor-param": ["@shape", "@file", "@default-file", "@name"],
    "to-signal": ["@max-frames"],
    "transpose": ["@axes"],
    "wavetable": ["@shape", "@file", "@name"],
    "wavetable-param": ["@shape", "@file", "@default-file", "@name"],
    "window": ["@type", "@N", "@n"],
    "windows": ["@shape"],
}


CURATED_OPERATOR_INPUTS = {
    "phasor": [
        {"name": "freq", "kind": "signal|float", "summary": "Frequency in Hz.", "required": True},
        {"name": "reset", "kind": "signal|float", "summary": "Optional reset trigger. Defaults to 0.", "required": False},
    ],
    "stateful-phasor": [
        {"name": "freq", "kind": "signal|float", "summary": "Frequency in Hz.", "required": True},
        {"name": "reset", "kind": "signal|float", "summary": "Optional reset trigger. Defaults to 0.", "required": False},
    ],
    "zeros": [{"name": "shape", "kind": "int[]", "summary": "Tensor shape, either as a bracket list or individual dimension arguments.", "required": True, "variadic": True}],
    "ones": [{"name": "shape", "kind": "int[]", "summary": "Tensor shape, either as a bracket list or individual dimension arguments.", "required": True, "variadic": True}],
    "randn": [{"name": "shape", "kind": "int[]", "summary": "Tensor shape, either as a bracket list or individual dimension arguments.", "required": True, "variadic": True}],
    "full": [
        {"name": "shape", "kind": "int[]", "summary": "Tensor shape, either as a bracket list or individual dimension arguments.", "required": True, "variadic": True},
        {"name": "value", "kind": "float", "summary": "Fill value.", "required": True},
    ],
    "tensor-param": [{"name": "shape", "kind": "int[]", "summary": "Tensor parameter shape, either as a bracket list, individual dimension arguments, or @shape.", "required": False, "variadic": True}],
    "triangle": [
        {"name": "phase", "kind": "signal|float", "summary": "Phase signal, usually 0..1.", "required": True},
        {"name": "duty", "kind": "signal|float", "summary": "Optional duty cycle width in 0..1. Defaults to 0.5; 0 follows the phase ramp.", "required": False},
    ],
    "wavetable": [{"name": "shape", "kind": "int[]", "summary": "Wavetable shape when not supplied with @shape.", "required": False, "variadic": True}],
    "wavetable-param": [{"name": "shape", "kind": "int[]", "summary": "Wavetable parameter shape when not supplied with @shape.", "required": False, "variadic": True}],
}

PREAMBLE_OPERATORS = [
    {
        "name": "polyblep_saw",
        "aliases": [],
        "category": "preamble",
        "summary": "Anti-aliased saw oscillator helper. Pass a phasor phase in 0..1 and frequency in Hz.",
        "signatures": ["(polyblep_saw phase freq)"],
        "arity": {"minimum": 2, "maximum": 2},
        "attributes": [],
    },
    {
        "name": "polyblep_pulse",
        "aliases": [],
        "category": "preamble",
        "summary": "Anti-aliased pulse oscillator helper. Width is clipped internally and should usually stay around 0.05..0.95.",
        "signatures": ["(polyblep_pulse phase width freq)"],
        "arity": {"minimum": 3, "maximum": 3},
        "attributes": [],
    },
    {
        "name": "svf",
        "aliases": [],
        "category": "preamble",
        "summary": "State-variable filter helper. Cutoff is Hz, q is resonance, mode is 0=LP, 1=BP, 2=HP, 3=notch, 4=peak, 5=allpass.",
        "signatures": ["(svf input cutoff q mode)"],
        "arity": {"minimum": 4, "maximum": 4},
        "attributes": [],
    },
    {
        "name": "ladder",
        "aliases": [],
        "category": "preamble",
        "summary": "Moog-style 4-pole low-pass helper with drive and resonance compensation. Cutoff is Hz, resonance is 0..1, drive is pre-filter saturation.",
        "signatures": ["(ladder input cutoff res drive)"],
        "arity": {"minimum": 4, "maximum": 4},
        "attributes": [],
    },
]


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


GLOBAL_ATTRIBUTES = {name: spec["summary"] for name, spec in ATTRIBUTE_SPECS.items()}


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
    "groups": {"type": "ManifestGroup[]", "required": False},
    "envelopes": {"type": "ManifestEnvelope[]", "required": False},
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
        "cellSpan": {"type": "int"},
        "default": {"type": "float"},
        "min": {"type": "float|null"},
        "max": {"type": "float|null"},
        "unit": {"type": "string|null"},
        "hidden": {"type": "bool|null"},
        "group": {"type": "string|null"},
        "env": {"type": "string|null"},
        "role": {"type": "string|null"},
    },
    "ManifestGroup": {
        "name": {"type": "string"},
    },
    "ManifestEnvelope": {
        "name": {"type": "string"},
        "group": {"type": "string|null"},
        "roles": {"type": "ManifestEnvelopeRoles"},
    },
    "ManifestEnvelopeRoles": {
        "attack": {"type": "string|null"},
        "decay": {"type": "string|null"},
        "sustain": {"type": "string|null"},
        "release": {"type": "string|null"},
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
        "activeCellId": {"type": "int"},
        "depthLanes": {"type": "ManifestModDepthLane[]"},
        "min": {"type": "float"},
        "max": {"type": "float"},
        "unit": {"type": "string|null"},
        "depthMin": {"type": "float|null"},
        "depthMax": {"type": "float|null"},
    },
    "ManifestModDepthLane": {
        "slot": {"type": "int"},
        "depthCellId": {"type": "int"},
    },
    "ManifestTensorInit": {
        "offset": {"type": "int"},
        "data": {"type": "float[]"},
    },
}


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def extract_function_attribute_usage(evaluator_source: str):
    fn_pattern = re.compile(r"\n\s*private func ([A-Za-z0-9_]+)\(")
    matches = list(fn_pattern.finditer(evaluator_source))
    attrs_by_fn = {}
    for idx, match in enumerate(matches):
        fn_name = match.group(1)
        start = match.end()
        end = matches[idx + 1].start() if idx + 1 < len(matches) else len(evaluator_source)
        body = evaluator_source[start:end]
        attrs = sorted(set(re.findall(r'attrValue\(attributes,\s*"(@[^"]+)"\)', body)))
        if attrs:
            attrs_by_fn[fn_name] = attrs
    return attrs_by_fn


def parse_case_names(case_text: str):
    return [name for name in re.findall(r'"([^"]+)"', case_text)]


def extract_operator_cases(evaluator_source: str):
    start = evaluator_source.index("switch op {")
    default_match = re.search(
        r"\n\s*default:\s*\n\s*throw LispError\.unknownOperator\(opName\)",
        evaluator_source[start:],
    )
    if default_match is None:
        raise ValueError("could not locate top-level operator switch default")
    end = start + default_match.start()
    block = evaluator_source[start:end]
    operator_map = {}
    lines = block.splitlines()
    idx = 0
    while idx < len(lines):
        stripped = lines[idx].strip()
        if not stripped.startswith("case "):
            idx += 1
            continue
        case_lines = [stripped]
        while ":" not in case_lines[-1]:
            idx += 1
            if idx >= len(lines):
                raise ValueError("unterminated case in operator switch")
            case_lines.append(lines[idx].strip())
        case_text = " ".join(case_lines)
        case_head, _, inline_body = case_text.partition(":")
        names = parse_case_names(case_head)
        if not names:
            idx += 1
            continue
        body_lines = [inline_body.strip()] if inline_body.strip() else []
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
        if re.search(r"\b(fn|op):\s*op\b", body):
            for name in names:
                operator_map[name] = {"aliases": [], "implementation": impl}
        else:
            operator_map[names[0]] = {"aliases": names[1:], "implementation": impl}
    return operator_map


def token_kind(name: str) -> str:
    lowered = name.lower()
    if lowered in {"channel", "slot", "n", "size", "hop", "hops", "max-hops", "maxhops", "rowindex"}:
        return "int"
    if lowered in {"shape", "axes", "ranges", "padding", "repeats"}:
        return "metadata"
    if "tensor" in lowered or lowered in {"a", "b", "input", "kernel", "prediction", "target"}:
        return "value"
    if lowered in {"signal", "sig", "input", "phase", "freq", "reset", "trigger", "ratio", "cutoff", "q", "gain", "mode", "condition", "delay", "time_in_samples"}:
        return "signal|float"
    if lowered in {"name", "type"}:
        return "symbol|string"
    return "value"


def describe_token(name: str) -> str:
    descriptions = {
        "a": "Left operand.",
        "b": "Right operand.",
        "x": "Input value.",
        "sig": "Input signal.",
        "signal": "Input signal.",
        "input": "Input value.",
        "freq": "Frequency in Hz.",
        "reset": "Optional reset trigger.",
        "trigger": "Trigger signal.",
        "phase": "Phase signal, usually 0..1.",
        "duty": "Optional duty cycle.",
        "cutoff": "Cutoff frequency in Hz.",
        "q": "Filter resonance / Q.",
        "gain": "Gain value.",
        "mode": "Mode selector.",
        "ratio": "Ratio value.",
        "threshold": "Threshold value.",
        "knee": "Knee value.",
        "attack": "Attack time.",
        "release": "Release time.",
        "sidechain": "Sidechain signal.",
        "time_in_samples": "Delay time in samples.",
        "channel": "One-based channel number in Lisp source.",
        "tensor": "Tensor input.",
        "kernel": "Convolution kernel tensor.",
        "index": "Lookup index.",
        "rowIndex": "Row lookup index.",
        "shape": "Tensor shape.",
        "value": "Value.",
        "hop": "Hop size in frames.",
        "condition": "Branch condition; nonzero means true.",
        "true_value": "Value returned when condition is true.",
        "false_value": "Value returned when condition is false.",
    }
    return descriptions.get(name, f"{name.replace('_', ' ')} input.")


def signature_tokens(signature: str):
    inner = signature.strip()
    if inner.startswith("(") and inner.endswith(")"):
        inner = inner[1:-1]
    return re.findall(r'"[^"]*"|\[[^\]]*\]|\S+', inner)


def attributes_from_signatures(signatures):
    attrs = []
    for signature in signatures:
        attrs.extend(token for token in signature_tokens(signature) if token.startswith("@"))
    return attrs


def infer_inputs_from_signatures(name: str, signatures):
    if name in CURATED_OPERATOR_INPUTS:
        return [dict(port) for port in CURATED_OPERATOR_INPUTS[name]]
    if not signatures:
        return []
    tokens = signature_tokens(signatures[0])
    if not tokens:
        return []
    inputs = []
    attr_mode = False
    for token in tokens[1:]:
        if token == "...":
            if inputs:
                inputs[-1]["variadic"] = True
            continue
        if token.startswith("@"):
            attr_mode = True
            continue
        if attr_mode:
            continue
        if token.startswith("[") or token.startswith('"'):
            continue
        inputs.append(
            {
                "name": token,
                "kind": token_kind(token),
                "required": True,
                "summary": describe_token(token),
            }
        )
    if name in {"noise", "window", "audio-tensor", "ir", "wavetable", "wavetable-param"}:
        return []
    return inputs


def apply_required_flags(inputs, arity):
    minimum = arity.get("minimum")
    if minimum is None:
        return inputs
    for idx, port in enumerate(inputs):
        if "required" not in port:
            port["required"] = idx < minimum
        else:
            port["required"] = bool(port["required"] and idx < minimum)
    return inputs


def result_kind_for_operator(name: str, category: str) -> str:
    if name in {"param", "in", "phasor", "stateful-phasor", "click", "ramp2trig", "accum", "latch", "mix", "biquad", "compressor", "delay", "peek", "to-signal", "overlap-add", "scale", "triangle", "wrap", "clip", "selector", "partitioned-convolve", "__modulated-param"}:
        return "signal"
    if name in {"tensor", "wavetable", "wavetable-param", "zeros", "ones", "full", "randn", "tensor-param", "audio-tensor", "ir", "matmul", "conv1d", "conv2d", "reshape", "transpose", "shrink", "pad", "expand", "repeat", "windows", "hann", "window", "softmax"}:
        return "tensor"
    if name in {"peek-row", "sample", "buffer", "spectrum-delay", "spectrum-delay-mod"}:
        return "signalTensor"
    if category in {"arithmetic", "math", "comparison", "conditional", "reduction"}:
        return "same-as-inputs"
    return "value"


def infer_outputs(name: str, category: str):
    tuple_outputs = {
        "fft": [("real", "tensor|signalTensor", "Real FFT bins."), ("imag", "tensor|signalTensor", "Imaginary FFT bins.")],
        "polar-fft": [("magnitude", "tensor|signalTensor", "Magnitude spectrum."), ("phase", "tensor|signalTensor", "Phase spectrum.")],
        "rect-fft": [("real", "tensor|signalTensor", "Real FFT bins."), ("imag", "tensor|signalTensor", "Imaginary FFT bins.")],
        "complex-mul": [("real", "tensor|signalTensor", "Real product bins."), ("imag", "tensor|signalTensor", "Imaginary product bins.")],
        "complex-conj": [("real", "tensor|signalTensor", "Real bins."), ("imag", "tensor|signalTensor", "Negated imaginary bins.")],
        "phase-vocoder": [("real", "signalTensor", "Real transformed bins."), ("imag", "signalTensor", "Imaginary transformed bins.")],
        "partition-ir": [("real", "tensor", "Partitioned IR real spectra."), ("imag", "tensor", "Partitioned IR imaginary spectra.")],
        "partitioned-spectral-mac": [("real", "signalTensor", "Real convolved spectrum."), ("imag", "signalTensor", "Imaginary convolved spectrum.")],
    }
    if name in tuple_outputs:
        return [
            {"name": port_name, "kind": kind, "summary": summary, "index": idx}
            for idx, (port_name, kind, summary) in enumerate(tuple_outputs[name])
        ]
    if name == "out":
        return []
    if name in {"def", "defmacro", "make-history", "make-tensor-history"}:
        return []
    return [{"name": "out", "kind": result_kind_for_operator(name, category), "summary": "Operator result.", "index": 0}]


def attribute_docs(names):
    docs = []
    for name in sorted(set(names)):
        spec = ATTRIBUTE_SPECS.get(name, {"type": "unknown", "summary": "Attribute observed in evaluator source."})
        doc = {"name": name, "type": spec.get("type", "unknown"), "summary": spec["summary"]}
        if "values" in spec:
            doc["values"] = spec["values"]
        if "aliases" in spec:
            doc["aliases"] = spec["aliases"]
        docs.append(doc)
    return docs


def build_attributes_index(operator_map, attrs_by_fn):
    usage = defaultdict(set)
    for name, meta in operator_map.items():
        fn_name = meta["implementation"]
        curated = CURATED_OPERATORS.get(name, {})
        discovered_attrs = (
            attrs_by_fn.get(fn_name, [])
            + attributes_from_signatures(curated.get("signatures", []))
            + CURATED_OPERATOR_ATTRIBUTES.get(name, [])
        )
        for attr in set(discovered_attrs):
            usage[attr].add(name)
    all_attrs = []
    for attr in sorted(set(GLOBAL_ATTRIBUTES) | set(usage)):
        spec = ATTRIBUTE_SPECS.get(attr, {"type": "unknown", "summary": GLOBAL_ATTRIBUTES.get(attr, "Attribute observed in evaluator source.")})
        all_attrs.append(
            {
                "name": attr,
                "type": spec.get("type", "unknown"),
                "summary": spec["summary"],
                "used_by": sorted(usage.get(attr, [])),
            }
        )
    return all_attrs


def load_preserved_preamble_operators(operator_output: Path):
    if not operator_output.exists():
        return []
    try:
        manifest = json.loads(read(operator_output))
    except (OSError, json.JSONDecodeError):
        return []
    operators = manifest.get("operators")
    if not isinstance(operators, list):
        operators = manifest.get("language", {}).get("operators", [])
    if not isinstance(operators, list):
        return []
    return [
        operator
        for operator in operators
        if isinstance(operator, dict) and operator.get("category") == "preamble"
    ]


def build_operators(operator_map, attrs_by_fn, preserved_preamble_operators=None):
    preserved_preamble_operators = preserved_preamble_operators or []
    operators = []
    for name in sorted(operator_map):
        base = operator_map[name]
        curated = CURATED_OPERATORS.get(name, {})
        category = curated.get("category", "uncategorized")
        signatures = curated.get("signatures", [])
        arity = curated.get("arity", {"minimum": None, "maximum": None})
        attrs = attrs_by_fn.get(base["implementation"], [])
        attrs = sorted(
            set(attrs)
            | set(attributes_from_signatures(signatures))
            | set(CURATED_OPERATOR_ATTRIBUTES.get(name, []))
        )
        inputs = apply_required_flags(infer_inputs_from_signatures(name, signatures), arity)
        outputs = infer_outputs(name, category)
        operators.append(
            {
                "name": name,
                "aliases": base["aliases"],
                "category": category,
                "summary": curated.get("summary", "Operator implemented in DGenLisp evaluator."),
                "signatures": signatures,
                "arity": arity,
                "input_count": arity,
                "output_count": {"minimum": len(outputs), "maximum": len(outputs)},
                "inputs": inputs,
                "outputs": outputs,
                "attributes": attrs,
                "attribute_docs": attribute_docs(attrs),
                "implementation": {
                    "function": base["implementation"],
                    "source_file": "LispEvaluator.swift",
                },
                "documentation": {
                    "source": "curated" if name in CURATED_OPERATORS else "source-discovered",
                    "port_docs": "curated-from-signature" if signatures else "not-yet-curated",
                },
            }
        )
    existing_names = {operator["name"] for operator in operators}
    for operator in [*preserved_preamble_operators, *PREAMBLE_OPERATORS]:
        if operator["name"] in existing_names:
            continue
        if all(
            key in operator
            for key in (
                "input_count",
                "output_count",
                "inputs",
                "outputs",
                "documentation",
                "implementation",
            )
        ):
            operators.append(dict(operator))
            existing_names.add(operator["name"])
            continue
        signatures = operator.get("signatures", [])
        attrs = operator.get("attributes", [])
        outputs = infer_outputs(operator["name"], operator["category"])
        operators.append(
            {
                **operator,
                "input_count": operator["arity"],
                "output_count": {"minimum": len(outputs), "maximum": len(outputs)},
                "inputs": apply_required_flags(infer_inputs_from_signatures(operator["name"], signatures), operator["arity"]),
                "outputs": outputs,
                "attribute_docs": attribute_docs(attrs),
                "implementation": {"function": None, "source_file": "generated preamble"},
                "documentation": {"source": "curated-preamble", "port_docs": "curated-from-signature"},
            }
        )
        existing_names.add(operator["name"])
    operators.sort(key=lambda operator: operator["name"])
    return operators


def build_operator_manifest(data):
    return {
        "schema_version": 1,
        "generated_from": data["generated_from"],
        "value_types": data["language"]["types"],
        "port_schema": {
            "inputs": "Regular Lisp arguments, excluding @attributes.",
            "outputs": "Result values available to patch-editor cables. Tuple-returning operators expose one output per tuple element.",
            "count": "minimum/maximum are regular argument or output counts. null maximum means variadic/unbounded.",
        },
        "operators": data["language"]["operators"],
        "attributes": data["language"]["attributes"],
        "special_forms": data["language"]["special_forms"],
        "constants": data["language"]["constants"],
    }


def main():
    parser = argparse.ArgumentParser(description="Generate structured DGenLisp API data for sequencer.")
    parser.add_argument("--dgenlisp-root", default=str(DEFAULT_DGENLISP_ROOT))
    parser.add_argument("--repo-root", default=str(Path(__file__).resolve().parents[1]))
    parser.add_argument("--output", default=None)
    parser.add_argument(
        "--operator-output",
        default=None,
        help="Patch-editor focused operator manifest output. Defaults to tools/dgenlisp-operators.json.",
    )
    args = parser.parse_args()

    dgen_root = Path(os.path.expanduser(args.dgenlisp_root)).resolve()
    repo_root = Path(args.repo_root).resolve()
    output = Path(args.output).resolve() if args.output else repo_root / "docs" / "dgenlisp-api.json"
    operator_output = (
        Path(args.operator_output).resolve()
        if args.operator_output
        else repo_root / DEFAULT_OPERATOR_MANIFEST
    )

    evaluator_source = read(dgen_root / "LispEvaluator.swift")
    operator_map = extract_operator_cases(evaluator_source)
    attrs_by_fn = extract_function_attribute_usage(evaluator_source)
    preserved_preamble_operators = load_preserved_preamble_operators(operator_output)
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
            "operators": build_operators(operator_map, attrs_by_fn, preserved_preamble_operators),
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
                    "__mod__<param>__active",
                    "__mod__<param>__depth_slot<N>",
                    "__mod__<param>__resolved",
                ],
                "required_param_attributes": ["@mod true", "@mod-mode", "@min", "@max"],
                "notes": [
                    "Modulatable params require at least one input marked with @modulator.",
                    "Generated modulation active and depth-lane params are hidden host parameters.",
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
        },
    }

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    operator_manifest = build_operator_manifest(data)
    operator_output.parent.mkdir(parents=True, exist_ok=True)
    operator_output.write_text(
        json.dumps(operator_manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(output)
    print(operator_output)


if __name__ == "__main__":
    main()
