# Sequencer Tour

The step grid is the fastest place to build repeating rhythms. Work on one track at a time, then play the tracks together. The inspector distinguishes the current step cursor from explicitly selected steps.

## Enter, select, and remove steps

Click an empty step to activate it. Click an active step once to select it for editing. Double-click an active step to remove its trigger. Clicking an active note once is therefore useful for p-locking without deleting it.

Use Left and Right to move the step cursor when no steps are selected, and Return to toggle the cursor's step. Up and Down change tracks in the step view. With a step selection, Left and Right shift the selected steps instead of merely moving the cursor.

Use Shift with the horizontal arrows to extend a selection. Shift-click selects a range; Command-click on macOS adds or removes individual steps. Check the selected-step count before turning a parameter. Backspace or Delete removes selected steps.

Dragging from an empty step paints triggers. Dragging an active step moves it. Holding briefly before sweeping across steps changes the gesture to selection. Start with individual clicks until painting, moving, and selecting feel distinct.

## Edit the musical result

- **Transpose** changes pitch relative to the track's base pitch.
- **Velocity** changes trigger strength; the instrument determines how that affects the sound.
- **Duration** changes note length. A sustaining synth makes the difference more obvious than a short drum envelope.
- **Pan** places the step left or right.
- **Retrig** adds repeated triggers inside a step; **Rate** controls their spacing.

For a simple variation:

1. Select every other hi-hat step.
2. Lower their velocity.
3. Select the final snare and add retriggers.
4. Listen before making another change.

An expanded track provides more step-level editing space and parameter lanes. The compact row is convenient for rhythms; the [Piano roll](piano-roll) is better for chords and precise lengths.

## Length and timing

The track settings include steps, timebase, swing, and voice behavior. **Steps** changes the pattern length. Longer patterns have multiple pages; confirm the page before assuming a note disappeared.

**Timebase** changes how steps divide musical time. Length and timebase are separate: sixteen steps do not always mean one bar. Tracks can have different lengths, so their rhythms meet differently on successive repetitions.

**Swing** delays alternating subdivisions; swing resolution sets the subdivision it acts on. Begin at the straight setting, then increase it while listening to a simple hi-hat pattern.

**Poly**, voice count, priority, and trigger behavior determine how overlapping synth notes are handled. Use polyphonic behavior and enough voices for chords and release tails. For a monophonic bass, overlap and glide can be part of the sound.

## Base values and p-locks

A selected step can have its own synth, effect, or mix parameter value. These are parameter locks, or p-locks. The cursor outline alone does not prove a step is selected: read the count. Clear selection before changing the pattern's ordinary value.

See [Parameter locks](parameter-locks) for editing, recording, indicators, and removal.

## Make a variation

In the mixer, select the track's pattern launch cell and use Command-D on macOS to clone the active track pattern. Edit that copy when you want an independent variation. The transport's scene **+** instead duplicates the project-wide scene.

Build another bass pattern while keeping the drums, or clone a scene for a complete alternate section. [Patterns and scenes](patterns-and-scenes) explains which operation to choose.
