# Audio effects

Audio effects process a track, a rack layer, a group, or a bus. The selected strip decides which chain you see and where a new effect lands.

## Add an effect

1. Select a track or bus.
2. Open **Audio FX** and double-click an effect, or drag it into the chain.
3. Play and adjust.

Str8 Delay, Filter, and EQ8 are good first choices. Put reverb on a send bus so several tracks can share it. Add one effect at a time.

## Order, bypass, remove

Audio flows through the chain in order. Drag effect headers to reorder. Dropping a new effect on an existing one inserts before it; dropping on the end area appends.

The enable button in each header bypasses the effect without removing it. Compare at matched loudness.

To remove an effect, click its header and press Delete or Backspace.

Track effects can drag between tracks. Bus and layer effects stay in their own chain; add a new one at the other destination instead.

## Scope

- **Track** effect: one track.
- **Layer** effect: one rack layer. **Track FX**: the summed rack.
- **Group** or **bus** effect: everything routed there.

Distort one bass layer and compress the whole rack, or group the drums and process the kit together. See [Racks](racks) and [Mixer](mixer).

## Locks and recall

Effect parameters take p-locks like synth parameters: select steps and turn the control, or record a knob move. See [Parameter locks](parameter-locks).

Track effect values follow the pattern. Bus and group effect values follow the scene. **Copy current values to all scenes** in a device header makes one setting global.
