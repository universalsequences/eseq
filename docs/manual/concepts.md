# Concepts

A track plays an instrument. Its pattern supplies the notes. Effects process the sound and the mixer routes it to the output. Scenes recall the whole project at once; the arrangement decides what plays over time.

## Tracks

A track is one musical part: a kick, a bass, a chord instrument. It has a sound source, a set of patterns, and mix controls.

Selecting a track shows its devices in the lower panel. Selecting is not arming: the **R** button arms a track for live input, the numbered button mutes it, and **S** solos it.

## Patterns

A pattern belongs to one track. It holds notes, step settings, p-locks, and that pattern's instrument, effect, and mix values. A bass pattern can be dark and quiet while another is bright and loud, with the drums untouched.

The step grid and the piano roll edit the same pattern. Nothing is copied when you open the piano roll.

## Scenes

A scene is project-wide. It picks one pattern per track and recalls bus and group settings. Two scenes that share a pattern share its edits; duplicate the pattern when you want an independent variation.

## Clips and takes

A clip is a placement on a track's arrangement lane. Its source is either a looping pattern or a **take**: a linear note performance recorded on the timeline. A take is notes, not audio. The piano roll's side panel says which one you are editing.

## The screen

- **Transport** at the top: Stop, Play, Record, position, tempo, quantization, and scene buttons.
- **Sidebar** on the left: Samples, Sounds, Kits, Instruments, Audio FX, MIDI FX, Presets, Projects.
- **Step grid**: one row per track, active steps lit in the track color.
- **Step inspector**: the current selection and its Transpose, Velocity, Duration, Pan, Retrig, and Rate.
- **Mixer**: track strips with pattern cells, sends, and buses.
- **Lower panel**: devices or the piano roll.
- **Arrangement view** replaces the grid with a scene lane and track lanes.

The upper-left controls show and hide panels. The upper-right controls switch between session and arrangement. See [Navigation and keys](customization).

## One transport

Session and arrangement share one transport. Launching a pattern or scene by hand while the timeline plays overrides it; **Back to Arrangement** hands control back.

The view you start recording in decides what you record. Session view overdubs the looping pattern. Arrangement view records a take. Switching views mid-pass does not change that. See [Recording](recording).
