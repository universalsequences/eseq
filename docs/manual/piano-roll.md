# Piano roll

The piano roll edits pitch, timing, duration, and automation.

## Open it

Double-click a track's name badge in the mixer, or press Shift-Tab to swap the lower panel. In the arrangement, double-click a clip's title bar.

The side panel names the source, a pattern or a take, and shows its length. **Loop on** means a pattern; **Loop off** means a take. Editing a pattern here changes every clip that uses it.

![The same pattern in the piano roll: pitch above, velocity below, and source settings at the side.](images/piano-roll.png)

## Notes

Pitch runs up the keyboard; time runs left to right. A note's left edge is its start and its width is its duration.

- Double-click empty space to add a note.
- Drag a note to change pitch or time. Drag its edge to change duration.
- Click or drag a rectangle to select. Backspace or Delete removes.
- Command-A selects all notes when the piano roll has focus.

## Clip window

An arrangement clip shows Start, End, and Offset. Start and End are the placement in the song. Offset is where in the source the clip begins. Change Offset to start a phrase mid-way; change the notes to change the music.

## Automation lane

The **Lane** selector below the notes picks a parameter: Velocity, Duration, Delay, Transpose, Pan, Retrig, Rate, and any device parameter that already has a p-lock in this pattern.

To automate a device parameter that is not listed, lock it once from the device panel or record a short knob move. It then appears in the menu, named as the engine names it, such as `inst lp_freq`.

A point marks each note's step and a segment spans its duration. Drag vertically to change the value.

- Colored points are locks. Gray points show the base value; dragging one creates a lock.
- Double-click a point to clear it.
- Chord notes on one step share one value. Locks belong to steps, not to voices.

Right-click the device control and choose **Clear p-locks** to remove every lock for a parameter. See [Parameter locks](parameter-locks).
