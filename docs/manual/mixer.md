# Mixer

The mixer balances tracks and routes them through groups and buses. Select a track or bus strip to show its devices. The colored name badge identifies the part; meters show signal level.

## Track controls

- The fader sets output level; Pan positions the track in stereo.
- Sends feed shared buses, such as A and B.
- The numbered button enables or mutes the track.
- **S** solos it; **R** arms it for live input and recording.
- The output selector chooses the destination, such as Main or an available group/bus route.

The cells higher in the strip launch track patterns. They are separate from mute, solo, and arm. Double-click the colored name badge for the piano roll.

Track mix values belong to patterns. After balancing one section, launch another pattern to check its own level and sends. A quiet variation can be intentional.

## Sends and buses

A send feeds some track signal to a shared bus. Several tracks can use one reverb or delay while retaining their direct output.

1. Select Bus A or B and inspect its effect chain.
2. Add or adjust the desired effect.
3. Return to a track and raise its corresponding send gradually.
4. Check bus and Main meters while listening.

A fully wet ambience return is often useful because the track already supplies dry sound. An insert on the track has a different wet/dry relationship.

## Group tracks

1. Select the first track in the mixer.
2. Shift-click another for a range, or Command-click strips/badges on macOS to toggle individual tracks into the selection.
3. With at least two tracks selected, press Command-G. The context menu also offers **Group Tracks** for suitable multi-selections.
4. Select the new group to inspect its strip and devices.

Groups combine member audio and provide shared processing and level. Collapsing hides members without deleting them. Members keep their own notes, patterns, and devices.

Use the group context menu to rename or ungroup. A group is not a layered instrument: its members remain independent sequencer parts.

## Group effects

Select the group itself, open Audio FX, and add processing. Confirm that the lower panel shows the group rather than the last member you edited. A group compressor hears the combined signal; a member compressor hears only that track.

Balance kick, snare, and hats before processing a drum group. If one drum dominates the compressor, reconsider member levels first.

## Scene-owned settings

Bus and group parameters are recalled by scenes. Track parameters are recalled by patterns. To give a chorus different group processing, select its scene before editing the group. To change one bass variation's level, edit its track pattern.

The arrangement scene lane recalls shared values while track clips follow their own lanes. See [Patterns and scenes](patterns-and-scenes).
