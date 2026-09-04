# Wavetable Bank

`bank.json` holds 28 wavetable sets x 16 waves x 512 samples (shape `[512, 448]`,
wave-major: `index = wave * 512 + sample`), generated procedurally by
`gen_bank.py`. Regenerate with `python3 gen_bank.py`.

The `sets` metadata below is the UI dropdown's source of truth. Its length and
order must match the `osc*_set` param range in ../dsp.lisp:

Basic Shapes, Harmonics, Sub, Saw Dual, Saw Harmonics, Pulse PW, Quad Saw,
Beating, 5th Brutal, Sync Additive, Sync Digital, FM Feedback, FM Fold,
FM Harmonics, Primes, No Primes, Galactica, Squeeze, Organ, Noise, Vowels,
Choir, Throat, Talk Box, Phoneme, Vox Sync, SFX Chaos, Bit Vox.

Every wave is DC-removed and peak-normalized to 0.85. The DSP indexes the bank
as `set * 16 + wave_position`; the `wavetable-viewer` UI widget reads the same
file directly for display.
