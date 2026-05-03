# Monomachine DPRO-WAVE V2 Mini Spec

## Source Notes

The Monomachine manual describes DigiPRO WAVE as a "crunchy waveform based synthesizer" with 32 original 512-byte 12-bit waveforms. Its synthesis page is:

- `WAVE`: selects one of 32 fixed waveforms.
- `WP`: wave phase, morphing from the selected waveform to the next waveform across 128 steps.
- `WPM`: wave phase modulation, a continuous sweep over the wave phase range; `0` disables the sweep.
- `WPRS`: wave phase restart; restarts the WPM sweep from `WP` on note-on when active.
- `SYNC`: off, `SFRQ`, or `PRCH`; hard sync source is either `SFRQ` or the previous channel.
- `SFRQ`: hard sync source frequency when `SYNC = SFRQ`.
- `TUNE`: oscillator tuning.

The manual also says user waveforms are only used by the MkII-only `DPRO-DDRW` and `DPRO-DENS` machines, while `DPRO-WAVE` uses its own 32 original waveforms.

Sources:

- Elektron Monomachine user manual, Appendix A, DigiPRO WAVE / Beat Box / DDRW / DENS.
- Elektron support notes on +Drive/Digibanks for Monomachine waveforms.

## Implementation Scope

`monomachine-dpro-wave-v2` implements a faithful DPRO-WAVE-inspired engine, not the whole DigiPRO family.

The engine models:

- one pitched oscillator
- 32 fixed generated 12-bit digital waveforms loaded from `waves/factory.json`
- a DGenLisp `wavetable` tensor with shape `[512 32]`, meaning samples x waves
- interpolated phase reads over the 512-sample cycle
- `WAVE` selection with wraparound next-wave morph
- `WP` base morph position plus optional `WPM` sweep
- `WPRS` note-trigger restart for the WPM sweep
- hard sync using `SFRQ`
- an approximated `PRCH` mode using a musically related internal sync source, because DGenLisp instruments do not receive previous-track oscillator phase
- `TUNE` as cents
- minimal amp/filter/output shaping so the synth is usable in this host without pretending those are DigiPRO synthesis parameters

## Parameters

Core:

- `wave`: 1..32, integer-ish selection
- `wp`: 0..127, wave phase/morph amount
- `sync_mode`: 0 off, 1 hard sync with fixed `sfrq` slave frequency, 2 hard sync with key-tracked `pitch + sfrq` slave frequency
- `sfrq`: 20..8000 Hz. In sync modes, played note pitch is the sync master/reset rate.
- `tune_cents`: -100..100 cents

Host shaping:

- amp ADSR
- lowpass cutoff/resonance/keytrack
- dedicated filter ADSR with bipolar cutoff amount
- drive and gain

UI behavior:

- The center ADSR editor shows the amp envelope by default.
- Selecting or editing the FILT panel switches the center ADSR editor to the filter envelope.

Modulation:

- `wp`, `sfrq`, `tune_cents`, `cutoff`, `resonance`, `filter_env_amt`, `drive`, and `gain` are modulation destinations.

## Non-Goals

- Exact factory waveform ROM reconstruction.
- BeatBox samples.
- MkII DDRW/DENS user waveform upload or Sysex/Digibank behavior.
- True previous-channel sync, until the engine can receive cross-track sync phase as an input.
