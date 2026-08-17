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
magnitude row -> cutoff resample -> contrast -> mirror -> IFFT
-> wrapped Hann IR bound -> FFT -> input STFT multiply -> IFFT/OLA
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

## Imported audio

Dropping a supported audio sample on the panel runs offline preprocessing:

1. deterministic channel downmix;
2. 64 evenly spaced 2048-sample frames (a shorter clip is periodically
   resampled as one cycle);
3. per-frame DC removal and RMS normalization;
4. forward FFT;
5. non-negative half-spectrum magnitude extraction and peak normalization.

Phase is deliberately discarded. Interpolating complex spectra would create
phase cancellation between adjacent frames and make frame morphing unstable.
Silent rows are retained as zero-magnitude rows; an entirely silent source is
rejected.

A fresh instance receives an original, reproducible procedural bank. No
third-party factory tables are included.

## Host behavior

Filter Table is registered alongside Convolution Reverb as a host-integrated
DGenLisp builtin. The host records the mutable tensor offset from the compiled
manifest and swaps a complete prepared bank at an audio block boundary.

Table references are persisted in projects and prepared data participates in
undo/redo mementos, allowing exact in-session replay without decoding the file
again. Project reload resolves user sources from the sample library by stem;
the procedural default requires no external asset.

## Known V1 limits

- The audio graph has no plugin-delay-compensation/latency-reporting API yet.
  Dry/wet alignment inside the effect is correct, but the 2048-sample effect
  latency cannot currently be reported to or compensated by the host.
- Response controls update once per 512-sample hop. There is no response-to-
  response interpolation yet, so very fast automation can zipper.
- Downward cutoff motion resamples the magnitude curve without a dedicated
  anti-alias prefilter.
- Minimum-phase/causal FIR mode is not implemented. A minimum-phase spectrum
  alone would still retain STFT latency; true zero-latency operation needs a
  separately profiled dynamic time-domain FIR path.
- Runtime processing uses the accelerated host FFT and therefore cannot compile
  for Metal. Removing `@backend accelerated` selects composed tensor FFTs for
  backend-portable experimentation.
