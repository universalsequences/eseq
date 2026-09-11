# Troubleshooting

Most surprises come from editing a different track, pattern, scene, or step selection than you meant to. Check those four first.

## Nothing plays

- The instrument finished loading, or the sampler has a sample.
- The pattern has active steps and Play is on.
- The track is not muted, nothing else is soloed, and the fader is up.
- The group, bus, and Main faders are up.
- MIDI effects are bypassed, in case one is eating the notes.
- In the arrangement, a clip covers the cursor. A scene marker alone is silent.

A moving track meter with a silent Main means a routing problem downstream.

## Keys do not play

Arm the track with **R**, leave any text field, and click a music panel. Try `Z` and `X` for the octave. On a drum rack, arm the header for pads or one member for pitches.

If letters play notes when you want to edit, disarm the track.

![Check R for live input, the numbered mute button, S for solo, and the fader for level.](images/armed-track.png)

## Recording landed in the wrong place

Session view records into the looping pattern. Arrangement view records a take. The choice is made when you press Record and does not change with the view. For knob recording, deselect all steps first.

## An edit only changed one step

Steps were selected. Command-click them to deselect, or check the inspector count, then edit again.

![A nonzero selected count means device edits apply to those steps.](images/selected-step.png)

## A knob jumps back

The parameter has p-locks; look for the marker. Edit or clear the locks. Also check what was launched: track values follow patterns, bus and group values follow scenes.

## A parameter is missing from the Lane menu

Device parameters appear after their first p-lock. Lock a step or record a short knob move, then reopen the menu.

## The timeline ignores a clip

**Back to Arrangement** is lit. Click it. Then check the clip's Offset and its source.

## A synth replaced my rack

Double-clicking in the sidebar replaces the selected track. Undo, then drag to **Drop sounds here** for a new track or into the rack's drop area for a layer.

## The export is empty or cut short

Clips must cover the exported range. Beat range counts beats from zero. Raise **Tail** if the ending is clipped. For WAV, turn it off to finish the file.

## The wrong thing was deleted

Delete acts on the selected object: an effect header, a pattern cell, notes, or an arrangement region. Undo, click the object you mean, and try again.
