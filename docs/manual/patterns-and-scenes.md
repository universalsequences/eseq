# Patterns and scenes

Patterns belong to tracks. Scenes belong to the project.

## What each one owns

A **pattern** owns its notes, step settings, p-locks, and its track's level, pan, sends, instrument, and effect values.

A **scene** picks one pattern per track and owns the bus and group settings. Groups have no patterns of their own; their processing lives in the scene.

If a track knob changes on its own, a different pattern was launched. If a bus knob changes, a different scene was.

## Launch a pattern

The small colored cells in a mixer strip are that track's patterns. Click one to launch it. Launch quantization in the transport decides whether the change is immediate or waits for the next boundary.

While stopped, clicking a cell sets the current scene's choice for that track. While the arrangement plays, a manual launch overrides the timeline until you press **Back to Arrangement**.

![The colored cells above the track name launch patterns for this track.](images/mixer-track.png)

## Launch a scene

The numbered buttons in the transport are scenes. Click one to recall every track's pattern and the bus settings. **+** duplicates the current scene; **-** removes it. Right-click the scene controls for more. The bank selector organizes larger projects.

![Scene buttons recall the whole project. The plus button duplicates the current scene.](images/scene-bank.png)

## Make a variation

- Duplicate a pattern: select its cell in the mixer and press Command-D. Edit the copy, or two scenes that share it both change.
- Duplicate a scene: press **+** in the transport. Use this for a whole new section.

Device headers offer **Copy current values to all scenes** when you want one setting everywhere.

## Scenes in the arrangement

The scene lane recalls bus and group settings over time. It does not place notes. Use **Place Scene Patterns** to write a scene's patterns into the track lanes, or place clips by hand. See [Arrangement](arrangement).
