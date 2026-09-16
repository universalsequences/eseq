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

## A MIDI keyboard or controller

Connect the device while eseq is running. New MIDI inputs connect automatically,
usually within a second. Arm a track or rack, then play a note.

Open **File → Settings…** to see the input devices and their connection status.
Use **Disable** or **Enable** beside a device to choose which inputs eseq accepts.
Choices are saved between launches on macOS; Linux choices apply to the current
session because its MIDI port IDs can change. **Refresh** checks the device list immediately.
Unplugging or disabling an input releases its held notes.

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

## Recover something you just played

Arm a track or drum rack and play freely with the transport stopped. The last
30 seconds of live keyboard, MIDI, and on-screen pad trigs are kept automatically;
Record and the metronome can stay off.

1. Run **M-x capture-midi** as soon as you hear a phrase you want to keep.
2. Drag across the roll to select its beginning and end. Each row shows one
   track and pitch. **Start (s)** and **End (s)** fine-tune the crop; **Zoom to
   crop** enlarges it.
3. Choose how many **Bars** the crop contains. Its length determines the BPM:
   a two-second crop with one bar gives 120 BPM. Slow tempos automatically double
   the bar count until the BPM reaches at least 70 (36 becomes 72; 48 becomes 96),
   preserving the phrase's timing. The displayed BPM is rounded
   to the nearest whole number, which is the project's current tempo precision.
4. If the transport is running, click **Stop playback**. Click **Loop crop** to
   audition through the original tracks' sounds at that BPM. The playhead shows
   your position. **Stop loop**, cropping, Refresh, or Cancel ends the preview.
5. Click **Send to tracks**. This sets the project to the displayed BPM and
   creates new patterns in the current scene's cells on the original tracks.
   Onsets and note lengths retain their proportions without quantization.
   Old patterns remain in the pattern pool. One Undo restores both the old
   patterns and the old tempo.

Opening the modal freezes the preview while live capture continues. **Refresh
capture** replaces that preview with the latest playing. Cancel leaves your
patterns untouched. Only notes starting inside the crop are included; notes
extending beyond its end are shortened there.

The preview uses the prepared notes and the tracks' MIDI and audio effects;
track processes and sequencer generators apply during normal pattern playback.
Capture follows live input before MIDI effects, and excludes sequencer playback
and roll-generated notes. It clears when you change projects. The buffer holds
up to 8192 trigs and reports if dense input exceeded that capacity. Patterns
currently share one velocity per step: import reports conflicting velocities,
excess notes in a step, or overlong notes instead of silently changing them.
Increasing the bar count can separate close hits into different steps.

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
