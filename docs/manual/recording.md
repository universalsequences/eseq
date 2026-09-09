# Recording

Recording notes into a looping pattern, printing parameter moves as p-locks, and capturing a linear arrangement are different tasks. The transport Record control is separate from track arming and from the **WAV** audio-output recorder.

## Record notes into a pattern

1. Open session/step-sequencer view and select the pattern you want to build.
2. Load a playable instrument or sampler.
3. Click the track's **R** in the mixer or record-arm circle in the row. Its arm indicator becomes active.
4. Click a music panel so a search box or text field does not own typing.
5. Play computer-keyboard notes before recording and confirm the meters respond.
6. Set record quantization in the transport.
7. Engage the red Record control and click Play. Perform the notes.
8. Stop, turn Record off, and inspect the pattern or piano roll. Disarm the track when finished with musical typing.

Session recording overdubs into the looping pattern. Each loop is another chance to add notes. Multiple armed tracks can receive input, so begin with one armed track.

## Computer keyboard layout

The white-note row is `A S D F G H J K L`, ascending from C through the next D. Black notes use `W E T Y U O`.

- `A W S E` play C, C-sharp, D, and D-sharp.
- `D F G H J` play E, F, G, A, and B.
- `T Y U` fill in F-sharp, G-sharp, and A-sharp.
- `K O L` continue with C, C-sharp, and D in the next octave.
- `Z` shifts the typing range down an octave; `X` shifts it up.

These are relative pitches: octave and instrument settings affect the actual register. Hold a key for a longer note and release it to end the held note. The instrument's release envelope can continue afterward.

Armed tracks give musical typing priority over some letter-based editing shortcuts. Disarm them when you want the letters to operate the editor.

## Record a drum rack

Arm the rack header to play the kit as pads. Each incoming pitch selects its assigned pad and plays that member at its base pitch. Arm an individual member track instead to play that sound chromatically. Arming the rack disarms its own members; arming a member disarms its rack. Other armed tracks can still receive input.

Match your typing octave to the pad notes shown in the rack. If a key addresses an empty pad, it produces no sound; use Z or X to reach the loaded pads. Audition first, then engage Record and Play. The member tracks receive the recorded rhythm on their own grids. See [Instrument racks](racks).

## Record quantization

Launch quantization and record quantization are separate transport selectors. Launch quantization determines when a pattern or scene launch happens. Record quantization determines how performed notes are placed.

Choose a subdivision such as 1/16 for grid-aligned input, or **off** to retain finer performance timing. Off does not prevent recording. Inspect the captured note positions and durations in the piano roll.

The MIDI-effect quantizer is another concept: it transforms playback events after the source pattern. It is not the recording setting.

## Record knob movements

Command-click selected steps to deselect them, or switch to another track and back. Confirm zero selected, engage Record and Play, and move a synth or effect parameter. While you manipulate the control, eseq prints values onto passing steps as p-locks. Release the control to end the gesture. Stop and turn Record off when finished.

This records step-based values, not an unrestricted continuous envelope. The knob follows your hand visually while the recorded result follows step timing. See [Parameter locks](parameter-locks).

## Record into the arrangement

Open arrangement view before starting the pass. Set the starting position, arm the track, engage Record, and play. The result is a linear **take** clip. Pattern and scene launches can also be captured on the timeline.

The recording kind is chosen at engagement and stays fixed through the pass. Opening arrangement halfway through a session overdub does not convert it into a take. Stop and begin again in the intended view. See [Arrangement](arrangement).

## Undo and audio recording

A recording pass is grouped for undo when it finishes. Stop first, then use Command-Z if you want to undo it. Other edits made during the pass may belong to the same transaction; save a project version before a long exploratory performance.

**WAV** and **File > Export Audio** concern audio output. See [Saving and audio export](saving-and-export). A MIDI take remains editable notes and uses the instruments and effects during playback.
