# Patterns and scenes

Patterns are track-based; scenes are project-wide. This determines what launches change and where parameter settings live.

## Pattern ownership

A track pattern holds notes, step settings, p-locks, and pattern-specific sound and mix values. That includes level, pan, sends, and instrument/effect parameters. It is more than a list of note-on steps.

A bass pattern can be quiet and dark, while another is louder and brighter, without changing the drums.

## Scene ownership

A scene chooses a pattern for each track and holds shared bus state, including group and master/bus effect values. Groups do not have ordinary note patterns of their own; their processing belongs to the scene-wide layer.

Scene 1 might choose Bass Pattern 1 and Drum Pattern 1 with short shared reverb. Scene 2 can choose Bass Pattern 2, keep Drum Pattern 1, and recall different bus settings. Editing that shared drum pattern affects both scenes.

## Launch a pattern

The small colored cells in a mixer strip launch that track's patterns. The transport launch-quantization setting determines whether the change is immediate or waits for a boundary. A queued indication can appear before the sound changes.

While stopped, choosing a cell sets the current scene's choice for that track. During playback, manual launches can override the arrangement. Back to Arrangement restores the timeline's choices.

A bass pattern cell is not a scene button and does not launch the whole project.

## Make independent variations

Select the pattern cell in the mixer and use Command-D on macOS to duplicate the active track pattern. Edit the duplicate when notes or sound should diverge. Reusing the original is useful when the part should stay identical.

The transport's numbered buttons are scenes. Click one for project-wide recall. **+** clones the current scene into another slot so you can build a whole alternate section. This differs from cloning only one track pattern.

The scene-bank selector organizes larger projects. Right-click scene/bank controls for available management actions. The **-** control removes a scene when removal is available. Save before reorganizing sections you want to keep.

## Mix recall

When adjusting a track, ask which pattern is active. When adjusting a group or bus, ask which scene is active. A level or cutoff that changes by itself may be recalled by a launch.

Use an available copy-current-values-to-all-scenes operation deliberately when you want consistent device values across sections. Do not assume one local edit is global.

## Arrangement scenes

The scene lane recalls shared scene and bus settings. Track clips specify the notes and track-pattern content that play. Setting a scene marker does not automatically place its track patterns.

Use **Place Scene Patterns** to write the scene's patterns into track lanes, or place individual clips. This lets group processing change while a bass clip continues underneath. See [Arrangement](arrangement).
