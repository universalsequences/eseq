# Recording

Three different things are called recording. The transport's red Record button records notes and knob moves. **WAV** records the audio output. Neither is track arming, which only decides where live notes go.

![Stop, Play, Record, and WAV. Record captures notes and knob moves; WAV captures audio.](images/record-controls.png)

## Record notes into a pattern

1. In session view, select the pattern to record into.
2. Arm the track with its **R** button.
3. Click a music panel so a text field does not own the keyboard.
4. Play a few keys and check the meters.
5. Set record quantization in the transport.
6. Enable Record, press Play, and perform.
7. Stop, disable Record, and disarm the track.

Each loop overdubs onto the last. Arm one track at a time until you want layered input.

![The lit R button arms this track for live input.](images/armed-track.png)

## The computer keyboard

```
 W E   T Y U   O
A S D F G H J K L
```

`A` is C. The top row is the black keys. `Z` and `X` move the range down and up an octave. The instrument's octave setting decides the actual register.

While a track is armed, letters play notes instead of running editor shortcuts. Disarm to get the shortcuts back.

## Record into a drum rack

Arm the rack header to play pads: each key triggers one pad at its base pitch. Arm a member track instead to play that one sound chromatically. Arming the rack disarms its members and vice versa.

Use `Z` and `X` to reach the loaded pads. Recorded notes land on each member's own pattern. See [Racks](racks).

## Record quantization

Record quantization snaps performed notes to a grid; **off** keeps your timing. It is separate from launch quantization, which schedules pattern and scene changes, and from the quantizer MIDI effect, which alters playback.

![This transport selector sets record quantization; off preserves your timing.](images/record-quantization.png)

## Record knob movements

Deselect all steps, enable Record, press Play, and move a control. Values print onto passing steps as p-locks. See [Parameter locks](parameter-locks).

## Record a take

Start in arrangement view. Set the cursor, arm the track, enable Record, press Play, and perform. Stop to finish. The result is a linear **take** clip in the track lane. See [Arrangement](arrangement).

The kind of recording is fixed when you press Record. Switching views mid-pass does not change it.

## Undo

Stop first. Command-Z then undoes the whole pass as one step. Save a version before a long exploratory take.
