# Racks

An **Instrument Rack** layers several sources on one track. A **Drum Rack** holds a kit of pads, each backed by its own member track. A **group** is neither: it only sums existing tracks for shared processing. See [Mixer](mixer).

## Build a layered instrument

1. Double-click **Instrument Rack** in the sidebar. A Layer Rack track appears.
2. Select it. The panel shows **Drop an Instrument or Sample**.
3. Drag an instrument or sample onto that area. Repeat for each layer.
4. Play the track and balance the layers.

The drop area adds a layer. Dropping on a selected layer's instrument area replaces that layer. Double-clicking a synth in the sidebar replaces the whole rack.

## Layers

Select a layer row to edit its instrument and effects. Each row has **T** transpose, **G** gain, **P** pan, **V** voices, plus **M** mute and **S** solo. The small toggles in the rack header show or hide the layer list, the selected chain, and the macros.

The track owns the pattern. Layers only decide what sounds.

## Layer effects and Track FX

Drop an audio effect on a layer row to process only that layer. **Track FX** processes the summed rack. A good first patch: a bright short layer with distortion, a darker sustained layer, and one shared delay after the rack.

## Macros

A macro is one knob that moves several controls.

1. Show the macro view with the rack's dial toggle and name a macro.
2. Click **map**. The number beside it counts destinations.
3. Click a parameter in any layer. It highlights when mapped.
4. Click **done** in the mapping sidebar. Turn the macro.

While mapping is armed, clicking a mapped parameter unmaps it. The sidebar sets each destination's Min, Max, and curve, and removes mappings with ×. Macros can be p-locked like any other control.

## Build a drum kit

1. Double-click **Drum Rack** and select the new rack.
2. Drag a kick, a snare, and a hat onto three empty pads.
3. Click a pad to hear it and edit its sound.
4. Enter steps on the member tracks: kick on 1 and 9, snare on 5 and 13, hats on 3, 7, 11, and 15.
5. Play and balance the member levels.

Each member is a full track with its own pattern, length, and effects. A drop lands on that pad's note; use the mini-map to reach pads beyond the visible grid.

## Play the kit

Arm the rack header to play pads from the keyboard, one pad per key. Arm one member to play that sound across pitches. Arming a rack disarms its members and vice versa. Use `Z` and `X` to reach the loaded pads. See [Recording](recording).

## Chokes, processing, and kits

Put closed and open hats in the same choke group so one cuts the other off.

Member effects process one drum; the rack's shared chain processes the whole kit. Member values follow patterns; shared chain values follow scenes.

Click the rack's save icon to save a **Kit**. A kit stores the rack and its pad sounds, not the patterns. Loading a kit replaces the selected drum rack, or creates one if none is selected.
