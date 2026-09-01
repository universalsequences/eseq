# Triton Wavetable Bank

`bank.json` holds 32 wavetable sets x 16 waves x 512 samples (shape
`[512, 512]`, wave-major: `index = wave * 512 + sample`, wave index
`= set * 16 + position_in_set`), extracted from the AKWF-FREE single-cycle
waveform collection by `extract_bank.py`.

Every wave is DC-removed and peak-normalized to 0.85 (matching the
convention used by `../../wavetable/waves/bank.json`).

## Provenance

Source material: **AKWF-FREE** (Adventure Kid Waveforms), public domain /
CC0 1.0 Universal.

- https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE

Each AKWF wave is a single-cycle waveform, 600 samples / 16-bit mono /
44.1kHz (verified against the actual WAV files in the collection; a small
`AKWF_stereo` folder exists but was not used). `extract_bank.py` resamples
each 600-sample cycle to 512 samples via FFT (rfft -> truncate/zero-pad bins
-> irfft), which is exact for periodic single-cycle material, then removes
DC and peak-normalizes to 0.85.

The AKWF-FREE clone itself (and its WAV files) is **not** part of this repo
-- only `extract_bank.py`, `bank.json`, and this README are checked in.

## Regenerate

```sh
git clone --depth 1 https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE.git /tmp/akwf
python3 extract_bank.py /tmp/akwf/AKWF
```

(or omit the argument to use the default scratchpad path baked into the
script). The script asserts on shape, peak, and DC on every run, so a
successful run is self-verifying. Wave selection per set is baked into
explicit AKWF filenames in `extract_bank.py`, so regeneration is
deterministic even if the upstream repo changes.

## Curation method

For each set, candidate waves were pulled from the relevant AKWF category
folder(s), spectral centroid was computed per wave (pulse duty cycle for the
square set), and 16 spectrally-distinct waves were picked spread across the
centroid range, ordered dark -> bright (or wide -> narrow pulse, for
"Square PW"). Large folders (organ, e-piano, FM synth, voice, strings) were
split into 2-3 sets by centroid rank so each Triton set stays a coherent
sub-flavor rather than spanning the whole folder's range.

## Set list (in order)

| # | Set | Description | AKWF folder(s) |
|---|-----|-------------|-----------------|
| 0 | Square PW | Pure square through progressively narrower pulse widths (wide -> narrow); powers the "Gliding Squares" PWM sweep sound | `AKWF_bw_squ` |
| 1 | Bright Saw | Edgy/bright band-limited sawtooth family | `AKWF_bw_saw`, `AKWF_bw_sawbright` |
| 2 | Round Saw | Softer, rounded/gapped saw variants | `AKWF_bw_sawrounded`, `AKWF_bw_sawgap` |
| 3 | Sub Sine | Near-pure sine through low sine-harmonic material, sub/fundamental-heavy | `AKWF_bw_sin`, `AKWF_bw_perfectwaves`, `AKWF_sinharm` |
| 4 | Tri Wave | Triangle-family single-cycle waves | `AKWF_bw_tri` |
| 5 | E.Organ Lo | Electric organ, darkest third by centroid (few drawbars) | `AKWF_eorgan` |
| 6 | E.Organ Hi | Electric organ, mid third by centroid | `AKWF_eorgan` |
| 7 | Organ Reed | Electric organ, brightest third by centroid (full drawbars/reedy) | `AKWF_eorgan` |
| 8 | E.Piano Soft | Electric piano / piano, darker half | `AKWF_epiano`, `AKWF_piano` |
| 9 | E.Piano Bell | Electric piano / piano, brighter/bell-like half | `AKWF_epiano`, `AKWF_piano` |
| 10 | Clavinet | Clavinet single-cycle waves, dark -> bright | `AKWF_clavinet` |
| 11 | Elec Bass | Electric bass waveforms, dark -> bright | `AKWF_ebass` |
| 12 | Dist Bass | Distorted/overdriven bass waveforms | `AKWF_dbass` |
| 13 | DX Bell | FM synth waveforms, darkest third (bell/EP-like) | `AKWF_fmsynth` |
| 14 | FM Metal | FM synth waveforms, mid third (metallic/inharmonic) | `AKWF_fmsynth` |
| 15 | FM Pluck | FM synth waveforms, brightest third (plucky/aggressive) | `AKWF_fmsynth` |
| 16 | Chip Digi | Chiptune-style digital oscillator waves | `AKWF_oscchip` |
| 17 | VGame Lead | Video-game-style lead waveforms | `AKWF_vgame` |
| 18 | Voice Ooh | Human voice formant waves, darker half (vowel-like) | `AKWF_hvoice` |
| 19 | Voice Choir | Human voice formant waves, brighter half (choir-like) | `AKWF_hvoice` |
| 20 | Str.Machine | String machine / bowed strings, darker half | `AKWF_stringbox`, `AKWF_violin`, `AKWF_cello` |
| 21 | Str.Bowed | String machine / bowed strings, brighter half | `AKWF_stringbox`, `AKWF_violin`, `AKWF_cello` |
| 22 | Grit Dist | Generic distorted/overdriven waveforms | `AKWF_distorted` |
| 23 | Nylon Gtr | Acoustic + electric guitar single-cycle waves | `AKWF_aguitar`, `AKWF_eguitar` |
| 24 | Reed Winds | Sax/clarinet/oboe/flute single-cycle waves | `AKWF_altosax`, `AKWF_clarinett`, `AKWF_oboe`, `AKWF_flute` |
| 25 | Overtone | Overtone-rich additive/drone material | `AKWF_overtone` |
| 26 | Odd Harm | Symmetric waveforms with strong odd-harmonic character | `AKWF_symetric` |
| 27 | Theremin | Theremin and tannerin sweeps | `AKWF_theremin` |
| 28 | Hand Drawn | Hand-drawn lo-fi waveforms, dark -> bright | `AKWF_hdrawn` |
| 29 | Bit Crush | Bit-reduced/quantized digital waveforms | `AKWF_bitreduced` |
| 30 | C64 Chip | Commodore 64 style chip waveforms | `AKWF_c604` |
| 31 | Granular | Granular/textural single-cycle waves | `AKWF_granular` |

The checked-in bank's `sets` metadata is generated from this list and is the
Triton UI dropdown's source of truth. Names stay max ~12 characters for the
dropdown.
