# Concepts and screen tour

Follow one sound through eseq: a track plays an instrument, its pattern supplies notes and parameter values, effects process the result, and the mixer routes it to the output. Scenes coordinate the project; the arrangement decides what happens over time.

## Tracks

A track is a musical part, such as a kick, bass, or chord instrument. It has a sound source, a collection of patterns, and mixing and processing controls. Selecting a track brings its controls into the device panel. Selecting a bus or group instead changes which effect chain that panel shows.

Selection, mute, solo, and record-arm are separate. Selecting a track lets you edit it. Arming it makes it a destination for live note input. The numbered mixer button enables or mutes it, `S` solos it, and `R` arms it.

## Patterns

A pattern belongs to one track. It contains notes, step parameters, p-locks, and pattern-specific instrument/effect and mix values. A bass variation does not have to change the drum pattern.

The step grid and piano roll edit the same musical content. A change in one appears in the other; opening the piano roll does not make a separate copy.

## Scenes

A scene is project-wide. It selects a pattern for each track and recalls scene-owned bus settings, including group processing. A verse and chorus can have different combinations of track patterns and different bus settings.

A scene references track patterns. If two scenes use the same pattern on a track, editing that shared pattern affects both uses. Duplicate the pattern when you want an independent variation. See [Patterns and scenes](patterns-and-scenes).

## Clips and takes

A clip is a placement on a track's arrangement lane. Its position and length say when its source plays. The source can be a repeating pattern or a take.

A take is a linear note performance recorded in the arrangement. It does not loop like a pattern. It is not a recording of the final audio output. The piano roll identifies whether you are editing a pattern or take and shows its loop behavior.

## The screen

- The transport across the top has Stop, Play, Record, position, tempo, quantization controls, and scene buttons.
- The sidebar browses Samples, Sounds, Kits, Instruments, Audio FX, MIDI FX, Presets, and Projects.
- The step grid shows a row per track, with active notes lit in the track's color.
- The step inspector shows the cursor/selection and controls such as Transpose, Velocity, Duration, Pan, Retrig, and Rate.
- The mixer shows track strips, pattern launch cells, sends, and buses.
- The lower panel shows devices or the piano roll, depending on what you open.
- Arrangement view replaces the session's main editing area with a scene lane and track lanes.

Panels can be hidden or resized. The upper-left controls toggle panels; the upper-right view controls switch session and arrangement. [Navigation and controls](customization) explains keyboard alternatives.

## One transport, two recording workflows

Changing views does not create a second transport. The arrangement can be playing while you look at the session grid. Manually launching a pattern or scene can take over from the timeline; Back to Arrangement returns control to it.

Start recording in session view to overdub looping patterns. Start in arrangement view to capture a performance on the timeline. Switching views during the pass does not change its recording kind. Stop and begin a new pass when you want the other kind.
