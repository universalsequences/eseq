# eseq Manual

eseq is a multi-track sequencer with its own synthesis, sampling and effects engine. Every track plays a sound that eseq generates itself. eseq accepts MIDI input from keyboards and controllers, but it does not send MIDI to external instruments.

This chapter describes what eseq can do and how its parts fit together, then names the areas of the screen. [The eseq concept](concepts) follows with the data model in detail and a worked example; [Your first session](first-session) then puts it into practice.

## What eseq can do

A project holds up to 64 **tracks**. Each track is one part: a sound source, a set of patterns, a MIDI effects chain, an audio effects chain and a mixer strip.

A **pattern** holds up to 256 steps, shown in pages of 16. Each pattern has its own length and timebase, so a 7-step bass line can run against a 16-step drum part. The timebase ranges from whole notes to 64ths, with triplet values and a **Prh** setting that fits the whole pattern into one bar, whatever its length. A step can hold a single note or a chord, and carries its own transpose, velocity, duration, pan, sync (wait for a grid line), delay, retrig count and retrig rate.

eseq has several ways to write, shape and generate notes, and they combine on the same track. Some write into a pattern; others act on it as it plays or generate notes of their own:

- **Step grid.** Enter and select steps on a grid of tracks, then edit their values in the step inspector or as one slider per step in an expanded track.
- **Piano roll.** Edit the same pattern by pitch, time and length, with an automation lane below the notes.
- **Live recording.** Play a MIDI keyboard or the computer keyboard into a looping pattern, with record quantize and a metronome. **Capture MIDI** keeps the last 30 seconds of live input, so a phrase played before recording started can still be kept.
- **Parameter locks.** Any instrument, effect or MIDI effect control can take a different value on individual steps. Locks can be set by selecting steps or recorded by moving a control while Record and Play are on.
- **Process lanes.** Every track has 14 per-step lanes that compute values while the pattern plays: probability, resets, random values, counters, accumulators, grabs from other tracks, comparators, vetoes, rolls and transposition. Their outputs can drive any step value or sound parameter.
- **MIDI effects.** A chain of up to four note processors per track. The factory set is **arp**, **beat-repeat**, **quantizer**, **spatial-harmonic-delay**, **transpose-range** and **trigger-to-track**.
- **Graph sequencers.** A network of nodes that fire and pass energy along weighted connections, sending notes to tracks. Track steps can seed the network, and a network can be attached to a Drum Rack. The factory **alez/neural** package provides one; attach it from the Packages tab.
- **Scripted sequencers.** Sequencers, per-step processes and whole editing views can be written in eseq's Lisp and shared as packages.

The sound engine provides:

- 40 factory instruments in three families: drums, physical models and synthesizers. All are compiled from DGenLisp source. **Create > Create Instrument…** opens the patch editor, where you build your own in the same language.
- A sampler that plays a sample whole, sliced at its transients, or warped to the project tempo, with a tagged sample library and sample import.
- **Instrument Racks**, which stack up to 16 instruments or samples on one track under 8 macro controls (created with **Create > Layer Rack**).
- **Drum Racks**, which group pads under one header. Each pad is a full track with its own pattern and effects.
- 20 built-in effects (delays, reverbs, compressors, filters, saturation and others) plus effects compiled from DGenLisp, up to 8 per chain.
- A mixer with sends, buses and groups. Buses, groups and Main each carry their own effect chain.

Finished work leaves eseq as a stereo WAV file through **File > Export Audio…**, or through the transport's WAV recorder.

## How the parts fit together

A **project** holds tracks; each track has a pool of patterns, and a pattern carries the track's sound and mix as well as its notes. **Scenes**, grouped in banks of up to 24, choose one pattern per track, and the **arrangement** places patterns and recorded takes on a timeline. [The eseq concept](concepts) sets out this model in full and traces it through a four-track example.

## The interface is Lisp

eseq's screens, menus and key bindings are written in its own Lisp and loaded when the app starts. The same language defines sequencers, process lanes, MIDI effects and packages. You can change a setting in **File > Customize…**, add a key binding with `bind-key` in `init.lisp`, or install a package that adds a sequencer or a view. The tracker view in the factory **alez/tracker** package (experimental) is one example: it adds a tab beside **Seq** that edits the same patterns in tracker columns. None of this is required for ordinary use.

## Find your way around

The main screen is the **session view**, where patterns loop and scenes are launched. The diagram shows its areas.

![Map of the session view. The browser sits beside the sequencer and mixer; the device panel runs across the bottom. Numbers match the descriptions below.](images/workspace.png)

1. **Transport.** Stop, play, record, the WAV recorder, Back to Arrangement, tempo, scene transpose, launch and record quantize, the metronome, roll mode, the scene buttons, and the master meters and CPU load. Three buttons at the far left show or hide the browser, the mixer and the device panel. Two buttons at the far right switch between the session view and the arrangement.
2. **Browser.** Tabs for samples, sounds, kits, instruments, audio effects, MIDI effects, presets, packages and projects.
3. **Step sequencer.** One row per track. The **Seq** tab is the step grid; packages can add further tabs beside it.
4. **Step inspector.** Transpose, velocity, duration, pan, retrig and retrig rate for the selected steps, with a count of how many steps are selected.
5. **Track settings.** Steps, timebase, swing and swing resolution, poly on/off and voice count, voice priority, retrigger or legato, mute group and scale.
6. **Mixer.** A strip per track with level, pan, sends, mute, solo and arm, and the pattern cells that launch patterns.
7. **Device panel** (Devices in the figure). The selected track's instrument, MIDI effects and audio effects. The same panel can show the piano roll instead.

Selecting a track points the step inspector, track settings and device panel at that track. The **File**, **Create** and **Pattern** menus are in the macOS menu bar.

## Reading this manual

The chapters follow the order in which you meet eseq. Concepts and the first session come first. Next come the ways of writing a pattern: step entry, locks, process lanes, the piano roll and recording. After those come the structures that organize patterns (scenes and the arrangement), then the sound: instruments, samples, racks, effects and the mixer. The last chapters cover saving, packages, customization and troubleshooting. Instructions use macOS controls and shortcuts.

- [The eseq concept](concepts) — projects, tracks, patterns, scenes and the arrangement, with a worked example
- [Your first session](first-session) — load an instrument, enter a four-note loop, lock one step, make a second scene, save
- [Step sequencer](sequencer-tour) — step entry, step values, pages, timebase, swing and voice settings
- [Parameter locks](parameter-locks) — per-step values for sound and effect parameters
- [Process lanes](process-lanes) — per-step probability, accumulators, grabs, comparators and rolls
- [Piano roll](piano-roll) — pitch, length, chords and automation for patterns and takes
- [Recording](recording) — live input, record quantize, Capture MIDI and knob recording
- [Patterns and scenes](patterns-and-scenes) — pattern variations, scene recall and scene banks
- [Arrangement](arrangement) — clips, takes, scene markers and the timeline
- [Instruments and presets](instruments) — the factory instruments, presets and building your own
- [Samples and sounds](sample-browser) — the sample library, the sampler and saved sounds
- [Racks](racks) — Instrument Racks, Drum Racks, macros and kits
- [MIDI effects](midi-effects) — the note-processing chain
- [Audio effects](effects) — effect chains on tracks, racks, groups and buses
- [Mixer](mixer) — levels, sends, buses and groups
- [Saving and export](saving-and-export) — projects and stereo WAV export
- [Packages](packages) — installing, sharing and writing packages, including the factory sequencers
- [Keys and customization](customization) — focus, shortcuts, buffers, Customize and init.lisp
- [Troubleshooting](troubleshooting) — silent tracks and unexpected behavior
