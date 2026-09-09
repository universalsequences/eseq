# MIDI effects

MIDI effects transform note events before they reach the sound generator. Audio effects transform the resulting sound. A MIDI effect can change rhythm, pitch, duration, or routing without processing the audio waveform.

## Add one

1. Select a playable instrument or sampler track.
2. Open **MIDI FX** and double-click an effect such as **arp**.
3. Confirm its panel appears and start playback.
4. Adjust one control at a time; use its enabled control to compare with the original notes.

Start with a sustained note or chord for an arpeggiator, and a simple rhythm for a repeat effect. Very short input notes may leave little time for repetitions.

## Arpeggiator

**arp** spreads the input notes into a sequence. Rate sets the subdivision, Direction changes ordering, Octaves extends pitch range, Gate changes generated note length, and Velocity changes their strength.

Create a chord in the piano roll, make it long enough for several generated notes, and begin at a slow rate. Then change direction or octave range. The effect transforms playback; it does not draw the generated output into your source piano roll.

## Other choices

- **beat-repeat** generates note repetitions. Rate controls spacing, Gate controls note length, and Velocity scales strength.
- **quantizer** places events on a selected timing grid. It is a playback effect, distinct from transport record quantization.
- **transpose-range** wraps pitches into a minimum/maximum transpose range; it is not simply a fixed transpose amount.
- **spatial-harmonic-delay** generates delayed note taps with pitch, velocity, and pan variation. The instrument articulates the generated notes again.
- **trigger-to-track** routes triggers to a chosen track. Set the track number and confirm the destination has a playable sound and appropriate level.

## Order and automation

Order matters: changing pitch before routing or repeating can differ from doing it afterward. Add effects one at a time until the event path is clear.

Supported MIDI-effect parameters use the same selected-step p-lock workflow. When a rhythm is unexpected, check the source pattern and MIDI effects before changing the synth. Bypass the MIDI effects to hear the untransformed input.
