# MIDI effects

MIDI effects change the notes before they reach the instrument. Audio effects change the sound afterward.

## Add one

1. Select an instrument track.
2. Open **MIDI FX** and double-click an effect.
3. Play the pattern and adjust. Use the enable button to compare with the plain notes.

Give an arpeggiator a held chord. Give a repeater a simple rhythm. Very short notes leave no room for repeats.

![The arp panel controls how held notes become an arpeggio before reaching the instrument.](images/arpeggiator.png)

## The effects

- **arp** plays the held notes one at a time. Rate, Direction, Octaves, Gate, and Velocity shape the result. Generated notes are not written into the pattern.
- **beat-repeat** repeats each note. Rate sets spacing, Gate sets length, Velocity scales strength.
- **quantizer** snaps playback to a grid. It is not the transport's record quantization.
- **transpose-range** folds pitches into a range between a minimum and maximum transpose.
- **spatial-harmonic-delay** adds delayed taps with pitch, velocity, and pan variation.
- **trigger-to-track** sends triggers to another track by number. Make sure that track has a sound.

## Order and locks

Effects run in chain order; transpose before repeat differs from repeat before transpose. MIDI effect parameters take p-locks like any other control.

When a rhythm sounds wrong, bypass the MIDI effects before touching the synth.
