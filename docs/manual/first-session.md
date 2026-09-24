# Your first session

This chapter builds a short synth part from an empty project. It follows the path from sound to pattern to scene that [Concepts](concepts) describes, one step at a time.

By the end you will have:

- loaded a factory instrument onto a track;
- entered a four-note pattern on the step grid;
- given one step its own pitch and its own filter setting;
- made a second scene with a variation of the part, and switched between the two;
- saved the project.

The walkthrough uses **Digi Drift**, a two-oscillator subtractive synth in the factory library. It enters notes only with the step grid and the step inspector. The other ways of making notes are covered in later chapters, and the last section of this one says where to find them.

## Start a project

Choose **File > New Project** (Command-N). If the current project has unsaved changes, eseq asks whether to save them first.

A new project contains:

- two empty tracks named **MIDI**, with no instrument loaded;
- one scene;
- two buses: Bus A with **Reverb** in plate mode, and Bus B with **Str8 Delay**;
- a tempo of 120 BPM.

Each track's pattern is 16 steps long at a timebase of 16th notes, so one pass of the pattern is one bar. Track 1 is selected.

The walkthrough uses the session view. If the middle of the screen shows a timeline, click the left of the two view buttons at the right end of the transport.

## Load an instrument

A track makes no sound until it has a sound source. Load one from the browser:

1. Click **Instruments** in the browser.
2. Click the search field (**Search instruments...**) and type `digi drift`. The factory instrument appears under **Synths**.
3. Double-click **Digi Drift**.

![Searching for Digi Drift narrows the browser to the matching factory instrument under Synths.](images/first-sound-browser.png)

The track takes the instrument's name, and Digi Drift's controls appear in the device panel. Its sections are the two oscillators and noise on the left, the envelopes in the middle, and the filter, LFO and pitch controls on the right.

![Digi Drift in the device panel. The FILTER section holds the Cutoff Hz control used later in this chapter.](images/digi-drift.png)

Double-clicking an instrument replaces the sound on the selected track. The track keeps its pattern. To add a new track with the instrument instead, drag it onto **Drop sounds here** at the end of the mixer. You can also drag an instrument onto an existing track to replace that track's sound.

## Enter four notes

The round buttons in the track's row are the pattern's 16 steps, numbered from 1 at the left. A lit step has a note on it.

Click steps **1**, **5**, **9** and **13**. Each click on an unlit step adds a note at C4 (Transpose 0).

![The Digi Drift track with notes on steps 1, 5, 9 and 13.](images/step-pattern.png)

Clicks on a lit step behave differently from clicks on an unlit one:

- A single click on a lit step selects it for editing. It does not remove the note.
- A double-click on a lit step removes its note.
- Dragging a lit step moves its note to another step.
- Dragging across unlit steps adds a note to each one.

The keyboard works as well. Up and Down select the previous or next track. With no steps selected, Left and Right move the step cursor and Return toggles the step under it. With steps selected, Left and Right move the selected notes instead. Shift-Left and Shift-Right extend the selection, and Backspace deletes the selected steps.

## Play the pattern

Press Space, or click **Play** in the transport. Press Space again, or click **Stop**, to stop.

![Transport playback controls. The square stops playback and the triangle starts it.](images/record-controls.png)

The playhead moves along the steps and Digi Drift sounds on steps 1, 5, 9 and 13: four quarter notes per bar. When it reaches the end of the pattern, playback returns to step 1. The meter on the track's mixer strip moves with each note. If the playhead moves but you hear nothing, see [Troubleshooting](troubleshooting).

Leave the pattern playing until the Save section. An edit is heard the next time the playhead reaches the step.

## Change one note's pitch

Click step 1 once. The step inspector shows **step 1 · 1 selected**. The count tells you how many steps the next edit applies to.

![The step inspector with one step selected. Transpose sets that step's pitch in semitones.](images/selected-step.png)

Drag the value beside **Transpose** up to **12**. Transpose is the step's pitch in semitones, where 0 is C4; the inspector reaches -48 to +48. At 12, step 1 plays C5, an octave above the other three steps.

The inspector's other values work the same way on the selected steps: **Velocity**, **Duration**, **Pan**, and **Retrig** and **Rate** for repeats within a step. They are covered in [Step sequencer](sequencer-tour).

## Give one step its own filter setting

Keep step 1 selected. In Digi Drift's **FILTER** section, turn **Cutoff Hz** down. Step 1 becomes darker. Steps 5, 9 and 13 keep their tone.

With steps selected, a device control writes a **parameter lock** on them; with none selected it sets the pattern's base value ([Concepts](concepts)). A small marker on the control shows that the parameter has locks in the pattern.

![Digi Drift with a cutoff lock on the selected step. The marker on Cutoff Hz shows that the parameter is locked.](images/locked-cutoff.png)

Press Esc to clear the selection (Command-clicking a selected step also deselects it). The inspector shows **0 selected**. Turn Cutoff Hz up. Steps 5, 9 and 13 get brighter, and step 1 keeps the dark value locked on it.

Locks are not limited to instrument controls. Effect parameters, MIDI effect parameters, rack macros, sends, timebase and swing can all be locked per step. [Parameter locks](parameter-locks) covers them in full, including recording locks by turning a control during playback and clearing them afterwards.

## Make a variation in a second scene

A scene points each track at one of its patterns. The project has one scene, shown as scene button **1** among the numbered scene buttons in the transport. The **+** and **-** buttons beside it add and remove scenes.

![The scene controls in the transport. The numbered scene buttons launch scenes; + adds one to the current bank.](images/scene-bank.png)

Click **+**. eseq creates scene **2** and makes it the current scene. **+** copies each track's pattern and sound into the new scene, so scene 2 starts identical and independent ([Concepts](concepts)).

Edit the copy in scene 2:

1. Double-click step 13 to remove its note.
2. Click steps 15 and 16 to add two notes that lead back into the start of the bar.
3. With no steps selected, turn Cutoff Hz further up.

Click scene button **1**. The original part returns: four notes, with the base cutoff you set earlier. Click scene button **2** to go back to the variation.

By default a scene switches at once, even partway through a bar. The launch-quantize menu in the transport reads **off** when eseq starts. Set it to **1 bar** and the next scene waits for the start of the next bar. The setting belongs to the session, not the project.

Scenes can also share a pattern; see [Patterns and scenes](patterns-and-scenes).

## Save the project

1. Choose **File > Save As…** (Command-Shift-S).
2. Type a project name and click **Save**.

![The Save As dialog. The project is saved into the projects folder as a single JSON file.](images/save-project.png)

The project file holds both scenes, their patterns and locks, the instrument settings, the buses and the tempo. Command-S saves; on an unnamed project it opens this dialog. Reopen the project from the browser's **Projects** tab (File > Open Project…, Command-O, opens it). Quitting with unsaved changes asks whether to save them.

If an edit goes wrong along the way, Command-Z undoes it and Command-Shift-Z redoes it.

## Where to go next

The part you built uses one way of entering notes and one kind of per-step value. The rest of the manual covers the others, roughly in the order you will want them:

- Step values in detail, pattern length, timebase (including triplets and Prh), swing, chords, voice settings and the Pattern menu: [Step sequencer](sequencer-tour).
- Every lockable parameter, lock recording, lock variants and the track lock table: [Parameter locks](parameter-locks).
- Per-step probability, accumulators, grabs from other tracks, comparators and rolls, computed as the pattern plays: [Process lanes](process-lanes).
- The same pattern edited by pitch, time and length, with an automation lane: [Piano roll](piano-roll).
- Live input from a MIDI or computer keyboard, record quantize, the metronome and Capture MIDI: [Recording](recording).
- Patterns and recorded takes laid out as clips on a timeline: [Arrangement](arrangement).
- Other factory instruments and presets: [Instruments and presets](instruments).
- Samples, slicing and drum kits: [Samples and sounds](sample-browser) and [Racks](racks).
- Arpeggiator and other note processors: [MIDI effects](midi-effects).
- Effect chains on the track, and the sends that reach this project's Bus A reverb and Bus B delay: [Audio effects](effects) and [Mixer](mixer).
- Graph and neural sequencers, the tracker view and other sequencers written in Lisp: [Packages](packages).
