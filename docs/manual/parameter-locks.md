# Parameter locks

A parameter lock, or **p-lock**, stores a parameter value at a sequence step. One note can have a different cutoff, effect amount, pitch, or other setting from the pattern's base sound.

Keep three things distinct: the base value, the value locked on a particular step, and the indication that locks exist somewhere in the pattern. Moving the base value does not erase locks.

## Lock selected steps

1. Stop recording while learning this workflow.
2. Select the intended track and pattern.
3. Click an active step once or select several steps. Check the selected-step count.
4. Change a synth or effect parameter. The edit applies to those steps as a p-lock.
5. Play and listen at the selected steps.
6. Clear selection before editing the base sound again.

For example, enter four bass notes, select the last one, and open its cutoff. The last note sounds brighter each time around. Select several steps to give them the same locked value.

To deselect a step without deleting its note, Command-click that selected step. Repeat for any other selected steps, or switch to another track and back to clear the selection. Confirm zero selected in the inspector. Double-clicking an active step removes its trigger, so do not use that gesture merely to deselect it.

Selection persists while you visit device controls. Clicking a knob does not automatically return to base editing. Check the count when an edit unexpectedly affects only part of a pattern.

## Record by moving a control

1. In session view, choose a pattern with notes.
2. Clear step selection: the inspector should show zero selected.
3. Engage Record and Play.
4. Press and move a synth or effect control while the pattern runs.
5. Release to finish the gesture. Repeat for another parameter or section.
6. Stop and turn Record off.

While printing, the control writes values to passing steps without replacing the ordinary base value. Printing lasts for the held gesture, not indefinitely after release. Selected steps retain their deliberate selected-step edit path, which is why clearing selection matters.

Audible and recorded changes follow step timing. Inspect them in the [Piano roll](piano-roll) automation lane afterward.

## Read the marker

A small colored marker at a synth or effect parameter means it has a p-lock somewhere in the current pattern. It does not necessarily mean the currently selected step is locked.

The displayed value can follow the inspected step or playback. Selecting another step or returning to the base state may change the display: that is recall, not necessarily a new edit. Stop playback when you want stable values to inspect.

## Clear a parameter's locks

1. Find the marked synth or effect parameter.
2. Right-click the parameter itself.
3. Choose **Clear p-locks**.
4. Confirm the marker disappears when no locks remain and replay the pattern.

This clears that parameter across the current pattern. It does not delete notes or every other parameter's locks. If your build also offers **Clear p-locks on selected steps**, that narrower option affects only the chosen steps. The unqualified action clears the whole pattern's locks for that parameter.

Use the piano-roll lane to clear one lock at a time: double-click its point. A device parameter returns to the base-value state; a step parameter resets to its default.

## When a control seems to ignore you

Check p-locks before reloading the instrument. Playback may recall a stored value immediately after a base edit. Edit the locked steps, print replacement values, or clear the parameter's locks.

Also check the owner. Track settings live with patterns; buses and groups live with scenes. A launch can recall different values even without a knob movement.
