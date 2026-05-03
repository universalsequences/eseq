# Monomachine DPRO-DDRW v1 Mini-Spec

## Source Notes

The Monomachine manual describes DPRO-DDRW as the MKII DigiPRO Doubledraw machine: an oscillator made from two waveforms. Unlike DPRO-WAVE's fixed 32-wave set, DDRW uses the 64-slot user waveform bank.

Manual parameters implemented here:

- `WAV1`: waveform slot 1, 1..64
- `MIX`: balance between the two waveforms
- `WAV2`: waveform slot 2, 1..64
- `TIME`: interpolation time when waveform slots change
- `BR1`: bit reduction for waveform 1
- `WID`: pitch distance between the two waveform oscillators
- `BR2`: bit reduction for waveform 2
- `TUNE`: oscillator tuning

Reference:

- Elektron Monomachine manual, Appendix A, DigiPRO Doubledraw section.

## Host Implementation

`monomachine-dpro-ddrw-v1` uses a file-backed 512 x 64 wavetable:

```lisp
(def waves (wavetable @shape [512 64] @file "waves/user-bank.json"))
```

The first oscillator reads `wav1` at played pitch. The second oscillator reads `wav2` at played pitch plus `wid`, mixed by `mix`.

`TIME` slews waveform slot changes. It does not smooth pitch, mix, or bit reduction.

`BR1` and `BR2` are implemented as 12-bit-style sample quantization amounts, with higher values reducing effective resolution.

## Added Host Shaping

Like DPRO-WAVE v2, this instrument includes host-side shaping so it is usable in this sequencer:

- amp ADSR
- dedicated filter ADSR with bipolar cutoff amount
- lowpass cutoff/resonance/keytrack
- drive and gain

The center ADSR editor shows the filter envelope when the FILT panel is selected, otherwise it shows the amp envelope.

## Non-Goals

- Exact Monomachine user waveform ROM or SysEx/Digibank behavior.
- Exact hidden parameter scaling for `TIME`, `BR1`, `WID`, or `BR2`.
- PRCH/neighbor track behavior; DDRW's manual parameter set here does not expose sync controls.
