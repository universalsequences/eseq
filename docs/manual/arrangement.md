# Arrangement

The arrangement puts music on a timeline: a scene lane for project-wide recall and track lanes for pattern and take clips. Build it with the mouse or capture a live performance.

## Orient yourself

Use the upper-right arrangement view control or Tab from a music view. The ruler shows bars, track names are at the left, and the thin lane above the tracks is the scene lane.

Click the ruler to set the arrangement cursor, which is the playback starting position. Check this, the selected track, and Record state before a pass.

## Place a pattern

1. Select the destination track.
2. Choose its pattern in the small selector beside **Place**.
3. Click Place. The interface prompts for a location.
4. Click the desired start in that track's lane.
5. Confirm the colored clip appears with the expected source.

Escape cancels before placement. Command-P is the macOS shortcut. The selector chooses a track pattern, not a project-wide scene.

You can also drag a track's pattern launch cell into its lane. It remains a use of the same source pattern: editing the source affects other uses too.

## Edit clips and sections

Select a clip to inspect it. Drag its body to move it and its edge to resize it. Double-click its title bar for the source piano roll. Placement Start/End and source Offset are separate from the underlying notes.

A longer pattern clip repeats its source. A take remains linear; placing it does not turn it into a repeating pattern. Duplicate the source pattern when another clip needs independent notes.

Select a region across the intended lanes and time span for section edits. Command-C, Command-V, and Command-D copy, paste, and duplicate arrangement selections on macOS. Check the selected region: it can include only part of a clip.

Delete acts on selected arrangement content. Undo reverses an edit; named project versions are useful before restructuring a song.

## Set scene state

**Set starting scene** chooses scene state at the beginning, including bus and group settings. It does not populate empty track lanes with notes.

Right-click the scene lane for **Set Scene Here**. An existing span also offers **Change Scene**, **Place Scene Patterns**, and **Remove Scene**.

Place Scene Patterns writes that scene's track-pattern choices into the lanes. Treat it as a clip edit that can replace content in the affected span. Merely changing or removing a marker changes scene recall without deleting independent track clips.

For example, keep a long bass clip continuous while scene changes above it recall different group reverb settings.

## Build an eight-bar section

For a first arrangement, use a one-bar pattern whose notes you already know.

1. Place it at the beginning of its track lane.
2. Drag the clip's right edge to the start of bar 9. Its one-bar source repeats across eight bars.
3. Repeat placement on the other tracks you want in the section.
4. Set the starting scene for the intended bus and group state.
5. Start playback at the beginning and check all eight bars.

To make the second half different, duplicate the source pattern first and edit that copy. Shorten the original placement to four bars, then place the variation at bar 5 and extend it to bar 9. Editing the original source alone would change every clip using it.

When this sounds right, save the project and use [Saving and audio export](saving-and-export) to make an audio file.

## Record a take

1. Open arrangement view before engaging recording.
2. Set the cursor at the desired start.
3. Arm the track and audition its sound.
4. Engage Record and Play, then perform with the computer keyboard or configured note input.
5. Stop to finish and turn Record off.
6. Find the new **Take** clip and open it in the piano roll.

The take starts at the punch-in position and has a linear length. Its panel identifies the take and shows Loop off. It is editable notes, not the finished audio waveform.

Recording edits the punched region while preserving content outside it. An overlapping pass is not an automatic stack of alternate comping lanes. Undo or return to a saved version if you want the previous result.

## Capture scene and pattern launches

Begin the recording pass in arrangement view, then launch scenes or individual track patterns as playback runs. Launches are captured at their musical positions; launch quantization still determines when queued changes happen.

Switching to session view during the pass lets you reach its launch cells. Recording remains arrangement capture because its kind was selected at the start. Stop to commit, then inspect scene and track lanes.

For a first trial, use two scenes and no armed note-input tracks. Begin capture, launch the first section, launch the second a few bars later, and stop. Replay from the beginning. Add live note takes once launch capture is familiar.

## Back to Arrangement

Manually launching scenes or patterns while the timeline plays can override scheduled content. The highlighted Back to Arrangement control near the transport indicates manual control. Click it to follow the timeline again.

Changing views alone does not cancel an override. If a clip seems ignored, check this control before editing the clip.

Starting on silence can automatically launch the selected scene for jamming. Playback can continue past arranged material, so a running clock does not prove a clip exists there.

## Four different objects

- A **pattern** is a track's reusable looping content and settings.
- A **scene** coordinates track-pattern choices and shared bus/group state.
- A **take** is a linear recorded note performance on one track.
- A **clip** is a placement referring to a pattern or take.

See [Recording](recording) for typing and quantization, and [Piano roll](piano-roll) for note and automation edits.
