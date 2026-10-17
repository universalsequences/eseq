# Beads

A port of the wavetable synth mode of Mutable Instruments **Beads**: the
continuous wavetable oscillator, the 32-grain wavetable granular player, its
trigger generator, the Griesinger reverb and the limiters. Ported from
`beads/dsp` in https://github.com/pichenettes/eurorack (commit `f4cfa769`).
The module's granular processing of live audio input is not part of this
instrument.

`waves/bank.json` is the module's `wav_integrated_waves` table: 12 banks of
16 pre-integrated waves, copied verbatim by `waves/extract_bank.py`. The table
also produces `waves/view.json`, the differentiated waves the panel's viewer
draws.

## Fidelity

The port was checked sample by sample against the original C++ `Synth`
compiled on the desktop. With randomization off, the oscillator, the grains
(every SIZE/SHAPE combination), bank, quality and grain stealing under
saturation match exactly, except that grains start one sample later. The
reverb matches to about 1% RMS error. The randomized controls have the same
distributions as the module's. The module's LCG correlates successive draws,
so combined size + shape randomization measures slightly wider and quieter
there than it does with an uncorrelated generator, which is what this port
uses.

Deliberate differences, all listed in `dsp.lisp`: the note gate is the SEED
gate, the note pitch replaces the fixed C3 root, an attack/release envelope
shapes the dry oscillator per note, and grain stealing prefers idle grains
over ones in their release tails.

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
instrument is an independent port, not affiliated with them.
