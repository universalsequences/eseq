# DPRO-DDRW User Bank

`user-bank.json` contains 64 single-cycle waves resampled to 512 samples for the DGenLisp wavetable loader.

Source material: Adventure Kid Waveforms Free, `AKWF--MonoMachine-SFX-60+`.

License: CC0-1.0. The AKWF project page states the waveforms are waived into the public domain to the extent possible under law.

Source:

- https://www.adventurekid.se/akrt/waveforms/adventure-kid-waveforms/
- https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE

The JSON stores data wave-major, matching the current DGen `peek` convention used by `wavetable-read`: `wave * 512 + sample`.

`user-bank-manifest.txt` records the AKWF source path for each of the 64 slots.
