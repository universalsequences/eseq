# Instruments and presets

Use Instruments in the sidebar to choose a sound generator. Factory instruments are organized into Drums, Physical Models, Synths, and other available categories. Library contains additional installed or user instruments. Search narrows the list.

## Add or replace

1. Open Instruments and expand a category, or search by name.
2. To create a separate track, drag the instrument to the empty **Drop sounds here** area in the mixer or arrangement.
3. To replace a track's sound, drop it on that track. Double-clicking a saved instrument also replaces the selected track when it supports replacement.
4. Wait for loading to finish, then confirm the track name and lower instrument panel.

The **Engines** section lists instruments already loaded in the project. You can drag an engine into Drop sounds here to create another track, or into a rack container to add a layer. Confirm the new track or layer before continuing.

A single browser click focuses an item; double-click or Return activates it. Choose your destination first. Double-clicking a synth while an instrument rack is selected can replace the rack. Adding a layer uses the rack's drop area instead.

The built-in Instrument Rack and Drum Rack entries create containers. Sampler plays a sample. These differ from saved synthesizers. See [Instrument racks](racks) and [Samples and sounds](sample-browser).

## Load a preset

1. Select the synth track you want to edit.
2. Open **Presets** in the sidebar.
3. Click a preset for that instrument. Its values appear in the panel.
4. Play notes or the pattern to judge the result.

Presets belong to instruments; selecting another instrument changes the relevant list. A preset is a starting sound, not a new sequence. Existing p-locks can still make individual steps differ from the loaded base sound.

For a fair comparison, stop recording, clear step selection, and check existing locks. Otherwise the pattern may recall values that obscure the preset's tone.

## Save your own preset

Use the small save icon in the instrument header to open **Save Preset**. Enter a name and choose **Save as New**. When a preset is loaded, **Overwrite: [preset name]** replaces that loaded preset. Typing a different name does not change which preset the overwrite button targets; choose Save as New to keep both sounds.

After saving, open Presets and confirm the new name appears for that instrument.

Save the project too. A reusable preset and the project containing your music serve different purposes. The project holds the sequence and arrangement that use the sound.

## Shape the sound

An oscillator or sound-source section supplies raw tone. A filter changes brightness and resonance. An amplitude envelope shapes the start, sustain, and ending of a note. Modulation moves parameters over time. Exact controls depend on the instrument.

Try shortening the amplitude envelope for a pluck, lowering cutoff for a darker tone, and increasing resonance slightly to emphasize the filter. Change one control at a time. Resonance or drive can change loudness as well as timbre.

Some instruments put envelopes or modulation behind tabs. A tab changes the visible controls, not the instrument instance. Check voice/polyphony settings when chords lose notes, and release time when notes ring after you stop playing.

Once the base sound works, use [Parameter locks](parameter-locks) for per-step variation and [Audio effects](effects) for processing.
