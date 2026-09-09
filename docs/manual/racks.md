# Instrument racks

An **Instrument Rack**, shown as **Layer Rack** in its device panel, layers sources under one track. A **Drum Rack** organizes percussion as pads and member tracks. A plain group combines existing independent tracks for shared processing.

## Build a layered instrument

1. Open Instruments and activate the built-in **Instrument Rack** entry. A new Layer Rack track appears.
2. Select it and find **Drop an Instrument or Sample** below.
3. Drag a saved instrument or sample into that rack container to create a layer.
4. Drop another source into the container to add another layer.
5. Play or sequence the rack track and balance its layers.

The destination matters. The container drop area adds a layer; the selected layer's instrument area replaces that layer's source. Double-clicking a saved synth in the sidebar can replace the selected track's instrument, including the rack, rather than adding a layer.

## Select and balance layers

The small rack view controls show or hide its layer list, selected chain, and macros. Check those toggles if a part of the panel seems missing.

Select a layer row to show its source and effects. The compact row labels are **T** for transpose/base note, **G** for gain, **P** for pan, and **V** for maximum polyphony. **M** mutes a layer and **S** solos it. Balance a bright attack layer against a darker sustained layer, and keep their combined level under control.

The track supplies the pattern; the selected layer determines which inner instrument you edit. Selecting a layer does not create a separate track pattern.

## Layer effects and Track FX

Drop audio effects onto a layer row or its layer-FX area to process only that layer. Use **Track FX** for processing after layers are combined.

For a first patch, combine one short bright source and one darker sustained source. Distort only the attack layer, then put a shared delay after the rack. Compare layers separately before adding more.

## Macros

Rack macros let one performance knob move several compatible controls. Start with a single destination so the relationship is easy to hear.

1. Show the rack's macro view using its small dial toggle.
2. Give a macro a useful name, such as Brightness.
3. Click its **map** button. The number beside map counts its destinations.
4. Click a compatible parameter in a rack layer, such as filter cutoff. Its mapping highlight shows that it is connected.
5. Click **done** at the top of the mapping sidebar to return to the browser, then turn the macro and check its effect. Clicking the armed map button also leaves mapping mode.

While mapping is armed, clicking an already mapped parameter removes that connection. Leave mapping mode before ordinary parameter edits. The mapping sidebar lets you inspect destinations, limit their Min and Max values, choose a response curve, or remove a mapping with its × control.

After one destination works, map a second layer's cutoff to the same macro. A narrow range can keep the combined sound useful throughout the knob's travel. Macros participate in the rack's pattern and p-lock workflows.

## Build a drum kit

1. Choose **Drum Rack** from Instruments and select the new rack.
2. Drag a kick sample or instrument onto an empty pad.
3. Drop a snare and a hat onto two other empty pads.
4. Click each loaded pad to trigger it and select its sound for editing.
5. Find the corresponding member tracks in the sequencer. Enter kick steps 1 and 9, snare steps 5 and 13, and hat steps 3, 7, 11, and 15.
6. Play the pattern and balance the member levels before adding shared processing.

This example assumes a 16-step straight-sixteenth pattern. Each member is a track with its own pattern, length, timebase, and effects. Selecting the rack does not combine their notes into one pattern.

The large pad grid shows part of the note range; use the mini-map to reach other pads. Empty positions remain empty until loaded. A drop targets that pad's note, so keep the pad assignments you intend to play rather than assuming sounds will be packed together automatically.

## Play pads or play one sound chromatically

Arm **R** on the rack header to treat incoming notes as pad choices. Each note addresses a pad and triggers that member at its base pitch. Match the computer-keyboard octave to the displayed pad notes; use Z and X to change octave.

Arm a member track instead when you want to play that one sound at different pitches. Arming a rack disarms its own members, and arming a member disarms its rack. This prevents the same member from responding both ways to one key. Unrelated armed tracks can still play.

Audition the keys before starting Record + Play. With rack arming, notes are recorded onto the addressed member tracks using each member's recording grid. See [Recording](recording).

## Process and save the kit

Use a member's effects for one drum and the rack's shared group/bus chain for the whole kit. Member-track parameters follow patterns; shared rack-bus parameters follow scenes.

For closed and open hats, assign the same non-Off choke group so one cuts off the other. A choke group controls sounding voices; it is different from muting a track in the mixer.

Select the rack and use its small save icon to open the Kits sidebar's save form. Enter a kit name and choose **Save Kit**. A kit stores the rack setup and its pad sounds, not the member patterns. Save the project to preserve the rhythms and arrangement too.

Activating a saved kit replaces the selected drum rack's kit. With no drum rack selected, activation creates a new rack. Check the destination before recalling a kit into a project you have already arranged.

## Rack or group?

Use an instrument rack when one played part should produce several layers. Use a drum rack when pads and separately sequenced percussion are useful. Use a group when existing independent tracks need a shared mixer destination. See [Mixer](mixer).
