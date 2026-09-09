# Audio effects

Audio effects process the output of an instrument or sampler. They can sit on a track, rack layer, group, or bus. The selected destination determines which chain is visible and where a new effect goes.

## Add an effect

1. Select the track or bus you want to process.
2. Open **Audio FX** in the sidebar.
3. Double-click an effect to add it to the selected destination, or drag it into the intended chain drop area.
4. Confirm that its panel appears below.
5. Play and adjust the effect.

Try Str8 Delay with modest wet amount and feedback, Filter or EQ8 for tone, or reverb on a shared send bus. Add one effect at a time so its contribution remains clear.

## Order and bypass

Audio flows through the chain in order. A filter before a delay shapes the input to the echoes; a filter after it shapes the resulting delayed sound too. Drag effect headers to rearrange the chain. Dropping a library effect onto an existing effect inserts it before that effect; dropping into the end area appends it.

Existing track audio effects can move between track audio chains. Bus effects reorder within their own bus, and rack-layer effects within their own layer. To use processing on another bus or layer, add an effect directly at that destination. MIDI effects belong to their separate note-processing chains.

The small enabled control in an effect header bypasses it without removing it. Compare at a similar perceived loudness: louder is not automatically better.

To remove an effect, select its header so the effect is the deletion target, then use Delete or Backspace. Check the selection first: deletion keys can also act on notes or other objects in their own contexts.

## Choose the processing scope

A track effect processes one track. In an instrument rack, a layer effect processes one selected layer; **Track FX** processes the combined rack output. A group effect processes all its member tracks after they are combined.

For example, distort one bass layer and compress the complete rack gently, or group several drum tracks and process the whole kit. See [Instrument racks](racks) and [Mixer](mixer).

## Parameter locks

Select steps and change a parameter to lock its value on those steps. With zero steps selected, engage Record and Play and move a supported control to print values onto passing steps. The parameter marker indicates that locks exist somewhere in the pattern.

Right-click a marked parameter and choose **Clear p-locks** to remove its locks. Moving the base value alone does not erase them. See [Parameter locks](parameter-locks).

## Recall across sections

Track effect parameter values follow the track's pattern. Bus and group effect values follow scenes. A scene change can alter a shared reverb or group processor even while you are concentrating on one track.

Some device headers provide **Copy current values to all scenes**. Use it deliberately when you want consistent settings across variations; it differs from editing only the currently recalled values.
