# Arrangement

The arrangement is a timeline. The scene lane at the top recalls scenes; the track lanes below hold clips. Build it with the mouse or capture a performance.

Switch views with the upper-right control or Tab. Click the ruler to set the cursor, which is where playback and recording start.

![The scene lane sits above the track clips. Clip previews show the notes placed on the timeline.](images/arrangement.png)

## Place a pattern

1. Select the track.
2. Pick a pattern in the selector beside **Place**, then click Place, or press Command-P.
3. Click where the clip should start in that track's lane. Place mode stays on so you can keep clicking; press Command-P, click Place, or press Escape to leave it.

You can also drag a pattern cell from the mixer into its lane. Either way the clip plays the same pattern; edit the pattern and every clip changes.

## Edit clips

Drag a clip to move it and its edge to resize it. A pattern clip loops its source to fill its length. A take clip is linear and does not loop. Double-click a clip's title bar to open its source in the piano roll.

Drag across lanes and time to select a region. Command-C, Command-V, and Command-D copy, paste, and duplicate it. Delete removes it. A region can cut through the middle of a clip.

## Scenes on the timeline

**Set starting scene** picks the scene at bar 1. Right-click the scene lane for **Set Scene Here**; right-click an existing span for **Change Scene**, **Place Scene Patterns**, and **Remove Scene**.

A scene marker recalls bus and group settings only. **Place Scene Patterns** writes the scene's patterns into the track lanes and replaces whatever was there. This split lets a bass clip run underneath while the reverb changes above it.

## An eight-bar section

1. Place a one-bar pattern at bar 1.
2. Drag its right edge to bar 9.
3. Do the same on the other tracks.
4. Set the starting scene.
5. Play from the top.

For a different second half, duplicate the pattern, edit the copy, shorten the first clip to four bars, and place the copy at bar 5.

## Record a take

1. In arrangement view, set the cursor and arm the track.
2. Enable Record, press Play, and perform.
3. Stop. A **Take** clip appears in the lane.

A take replaces what was in the recorded region and leaves the rest alone. It is editable notes, not audio.

## Capture launches

Start recording in arrangement view, then launch scenes or pattern cells while it plays. Each launch lands on the timeline at its quantized position. Switch to session view to reach the cells; the pass stays an arrangement recording.

Try it first with two scenes and no armed tracks.

## Back to Arrangement

A manual launch during timeline playback overrides the timeline and lights **Back to Arrangement** by the transport. Click it to follow the timeline again. Changing views does not clear the override.

Playback runs past the last clip, so a moving cursor does not mean something is placed there.

## Four objects

- A **pattern** loops and belongs to a track.
- A **scene** picks patterns and holds bus and group state.
- A **take** is a linear recorded performance.
- A **clip** places a pattern or take on the timeline.
