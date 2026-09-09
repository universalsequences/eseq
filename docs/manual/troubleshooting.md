# Troubleshooting

Start with the selected track, current pattern, current scene, and Record state. Many surprises come from editing or playing a different scope than intended.

## A silent pattern

- Confirm the instrument finished loading or the sampler has a sample.
- Check active steps and Play.
- Check track enable/mute, solo elsewhere, level, and output route.
- Check group, bus, and Main levels too.
- Bypass MIDI effects to check whether input is suppressed or routed elsewhere.
- In arrangement view, confirm a track clip covers the playback position. A scene marker alone does not place notes.

A moving track meter with no Main signal suggests checking downstream routing. No track response suggests notes, loading, envelopes, or voice settings.

## Keys do not play notes

Arm with the track's R control, leave search/text fields, and click a music panel. Check the octave with Z/X and use a known playable preset. Selecting a track alone does not arm it.

For a drum rack, check whether the rack header or an individual member is armed. Rack arming selects pads by note; member arming plays one sound chromatically. A key aimed at an empty pad stays silent.

If letters play notes while you want to edit, disarm the track.

## Recording went to the wrong place

Start in session view for looping overdubs and arrangement view for takes and launch capture. Recording kind remains fixed through a pass even when views change.

Clear step selection for knob recording. Selected steps retain deliberate p-lock editing rather than becoming a moving target. Check Record and Play are both active. Stop before undoing a pass.

## An edit changes only one step

Check the selected-step count. Selection persists while you edit device controls, and clicking the same track again may leave it intact. Command-click selected steps to deselect them, or switch to another track and back. Confirm zero selected before editing the pattern's base sound.

## A knob returns to another value

Check its p-lock marker. Playback may be recalling a step value. Edit the locks, or right-click and choose Clear p-locks. Moving the base setting does not erase automation.

Also check launches: track values follow patterns, while bus/group values follow scenes. Loading a preset does not mean all locks disappeared.

## A device parameter is absent from Lane

The piano-roll menu lists device parameters after they have at least one lock. Create one on a step or record a short gesture, then reopen the selector. Removing the last lock can remove the entry again.

## The timeline ignores a clip

Check Back to Arrangement: manual launches can still override the timeline while arrangement view is visible. Inspect the source, offset, and placement too. Moving a clip and editing source notes are separate operations.

## A synth replaced a rack or track

Sidebar activation can replace the selected compatible track. Use the empty-track drop area for a separate track and the rack container for a layer. Undo, then choose the destination explicitly.

## An audio export is empty or cuts off early

Check that clips occupy the requested arrangement range. A scene marker alone does not supply notes. Beat-range fields count from zero in beats, not bars. Increase Tail if the final delay or reverb has not decayed. For a live WAV capture, turn WAV off to finish the file. See [Saving and audio export](saving-and-export).

## The wrong object was deleted or duplicated

Shortcuts follow context and selection. A device header, pattern cell, note selection, and arrangement region are different targets. Undo, click the intended object, confirm its selection, and repeat once.

Use File > Save As for milestones before major edits. Named versions are easier to revisit than reconstructing an exploratory session.
