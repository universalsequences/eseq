# Piano roll

The piano roll edits pitch, timing, and duration. It is useful for melodies, chords, recorded performances, and p-lock automation.

## Open the right content

Double-click a track's colored name badge in the mixer. The lower panel shows a keyboard, notes, source information, and an automation lane. Shift-Tab also switches the lower main/device panel and piano roll.

In arrangement view, double-click the clip's title bar to edit. Read the source panel first: it distinguishes patterns from takes and shows length and loop state. Editing a shared pattern changes its other uses too.

## Edit notes

Pitch runs vertically beside the keyboard; time runs horizontally. Each bar is a note, with its left edge at the start and its width representing duration. Notes at the same time form a chord.

- Double-click empty space to create a note.
- Drag its body to change pitch or timing.
- Drag an edge to change duration.
- Select notes individually or with a selection rectangle, then use Backspace or Delete to remove them.
- Command-A selects notes in the focused piano roll on macOS, rather than step cells in the grid.

Zoom and scroll to give short notes enough space. For recorded performances, inspect starts and ends rather than assuming all bars are aligned to the grid.

## Patterns and clip windows

A pattern loops; a take is linear. The side panel shows **Loop on** for a pattern and **Loop off** for a take. Arrangement clips can expose Start, End, and Offset: placement in the song and position within the source are separate.

Changing Offset changes which part of the source plays first. Changing source notes changes the music itself. Keep the operations separate when a phrase appears to begin in the middle.

## Automation lane

The **Lane** selector below the notes chooses a parameter. Basic choices include Velocity, Duration, Delay, Transpose, Pan, Retrig, and Rate. Device parameters appear after they have at least one p-lock in the current pattern.

To edit a cutoff missing from the menu:

1. Return to the synth controls.
2. Select a step and change cutoff to create a lock, or record a short cutoff gesture.
3. Reopen the piano roll and Lane menu.
4. Choose the cutoff entry. It may use a parameter name such as `inst lp_freq` rather than the panel's friendlier label.
5. Edit its values in the lane.

This first-lock requirement keeps the list focused on parameters actually in use. An absent device parameter is not necessarily unautomatable.

## Change and clear lane values

A point marks a note onset and a horizontal segment spans its duration. Drag the point or segment vertically to change the value. This edits the parameter at that step; it does not move the note's pitch.

Colored device points represent locks. Gray points show the base value where no lock exists. Dragging a gray value creates a lock. Double-click a point to clear it. For a basic step parameter such as velocity, clearing resets the default instead.

P-locks belong to steps. Chord notes on one step share step-level automation; the lane is not an independent synth-parameter envelope per chord voice.

To remove every lock for a parameter, use **Clear p-locks** on its device control. Removing its last lock can remove the device parameter from the Lane menu. See [Parameter locks](parameter-locks).
