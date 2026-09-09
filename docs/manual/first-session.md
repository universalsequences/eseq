# Your first session

This walkthrough makes a short synth pattern, gives one note a different tone, adds an effect, and saves the project. Digi Drift is the example; another installed synth also works.

## Choose a sound

1. Save any project you want to keep, then choose **File > New Project**.
2. Open **Instruments** in the sidebar and expand **Synths** under Factory.
3. Select an empty MIDI track and double-click **Digi Drift**. Wait for loading to finish. Its name should appear on the track and its controls below.
4. Open **Presets** and click a preset, such as Acid Squelch. Its values appear in the instrument panel.

Double-clicking a saved synth can replace the selected track's instrument. To add a separate track, drag the instrument to **Drop sounds here** instead. See [Instruments and presets](instruments).

## Punch in notes

1. Click empty steps 1, 5, 9, and 13 on the synth row. Four lit steps should appear.
2. Click Play. The pattern repeats and the track/output meters should move.
3. Click Stop, then click the already active step 9 once. Check that the inspector says **1 selected**.
4. Change Transpose to give that note a different pitch. The other notes keep their pitch.

On the default 16-step straight-sixteenth pattern, these steps mark the four beats of a bar. Changing length or timebase changes that relationship.

## Give one note a different tone

1. Keep step 9 selected and turn the synth's filter cutoff.
2. Play the pattern. That step uses the locked cutoff value.
3. Look for the parameter's p-lock marker. Right-clicking the parameter exposes **Clear p-locks**.
4. Stop, then Command-click selected step 9 to deselect it. Verify that the selected-step count is zero before editing the overall sound. Clicking the same track again does not necessarily clear its selected steps.

Selected steps receive parameter locks. An ordinary edit with no step selection changes the pattern's base value. [Parameter locks](parameter-locks) covers this and recording knob gestures.

## Add delay and save

1. Select the synth track, open **Audio FX**, and double-click **Str8 Delay**.
2. Find the delay in the lower chain. Play and adjust its wet amount and feedback.
3. Use the small enabled control in its header to compare processing with bypass.
4. Choose **File > Save As**, enter a project name, and press Save.

The project is saved into the app's projects folder. Use File > Save for later changes, and Projects in the sidebar or File > Open Project to reopen it.

Continue with [Recording](recording) to play notes instead of clicking, or [Arrangement](arrangement) to put the pattern on a timeline.
