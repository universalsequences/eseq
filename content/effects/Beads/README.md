# Beads

A port of Mutable Instruments **Beads** as an audio effect: the granular
processor that records its input into a looping buffer and plays it back
with up to 32 grains, the multitap delay and beat-slicing looper it turns
into with SIZE fully clockwise, the feedback path, the four recording media,
the Griesinger reverb and the limiters. Ported from `beads/dsp` in
https://github.com/pichenettes/eurorack (commit `f4cfa769`). The wavetable
synth mode is the separate `Synths/Beads` instrument.

`dsp.lisp` is hand-written; `data/ulaw.json` is the module's 8-bit mu-law
decoder (`tools/beads/tools/extract_ulaw.py`). Verification tooling is in
`tools/beads`.

## Controls

- **time / scatter, size, shape, pitch** and their ± amounts: the module's
  TIME / SIZE / SHAPE / PITCH knobs and attenuverters.
- **density**: regular grains below noon, random above, none at noon (only
  SEED presses).
- **seed**: the SEED button. A rising edge fires a grain; with **latch** off,
  grains only flow while it is held, and in delay mode it taps the tempo.
  **latch** on is the module's latched SEED (free-running).
- **freeze**, **feedback**, **dry/wet**, **reverb**.
- **media**: dirty tape (24 kHz, 8-bit mu-law, 16 s, wow & flutter), clean
  tape (24 kHz, 12-bit, 10.7 s), vintage (32 kHz, 12-bit, 8 s), hi-fi
  (48 kHz, 16-bit, 4 s).
- **input / output**: manual input gain (the module's gain setting) and
  output level.

## Fidelity

Checked sample by sample against the original `Processor` compiled on the
desktop (`tools/beads/ref`, `tools/beads/tools/compare.py`), in hi-fi with
randomization off: dry path, grains (all size/shape/pitch combinations
tested, reversed, frozen mid-stream, with feedback), SEED taps, and delay
mode (free and tapped clocks, pitch-shifted taps, LP/HP filter, frozen
slices) match to 0.0–0.2% RMS error; the reverb to 0.5%.

The other media keep their bit depth, recording time and time constants but
run at the host rate, band-limited to the media's codec rate instead of
resampled; levels match the original within 0.5 dB (`lofi.py`).

Deliberate differences:

- **No auto gain.** Clean tape's level-dependent soft clipping follows a
  peak follower of the gained input.
- **Grain stealing** uses the module's slow-rate rule (least remaining time)
  at every rate, and its four safety fades land over four samples.
- **SEED presses** fire a grain even while latched.
- **Stereo only**: no mono 2x-time mode, no gate output.
- Recording time is fixed in seconds; above 48 kHz the buffer holds 16 s at
  the host rate at most.

Cost: about 14% of one core (32 grains with Hermite reads and the eight
delay taps all run every sample, whichever mode is active).

## License

The ported DSP and the wavetable data are by Emilie Gillet, MIT licensed:

> Copyright 2017-2021 Emilie Gillet.
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in
> all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
> THE SOFTWARE.

"Mutable Instruments" and "Beads" are names of the original hardware; this
effect is an independent port, not affiliated with them.
