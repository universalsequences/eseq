# Manual v0.1 authoring and verification record

Date: 2026-09-09. Content bead: `eseq-ug3m.5`. Human review: `eseq-ug3m.8`.

The first pass is 18 pages, approximately 10,000 words, under `docs/manual/`.
It replaces the placeholder chapters and covers the graphical music-making
scope requested by the user. Packages, custom scripts, custom sequencers,
scripting, and patcher authoring are excluded. The existing `customization`
node now explains navigation and controls rather than Lisp customization.

## Method and limits

The running `/Applications/ESeq.app` was inspected and operated through the
computer-use API. Its custom-drawn UI exposes almost no accessibility elements,
so interaction used screenshots and coordinates. Source was read to check
semantics, especially where older design documents describe superseded behavior.
No application implementation was changed for the walkthrough.

The Mac locked during the later pass. No attempt was made to bypass the lock.
Browser drag-and-drop attempts before that did not reliably complete through
computer use. This is not enough evidence to label drag-and-drop an app defect.
The manual contains source-supported drag workflows, with their incomplete
hands-on verification explicitly recorded here and in `eseq-ug3m.8`.

The development checkout contains concurrent, unrelated edits. It is not
assumed byte-identical to the installed build. In particular, the running
parameter menu showed the unqualified Clear p-locks action; source also supports
selected-step-only clearing. The chapter phrases the narrower action conditionally.

## Observed in the running app

- File > New Project, then Instruments > Factory > Synths; double-clicking Digi
  Drift replaced the selected empty MIDI track and displayed its synth panel.
- Four-step entry, playback with active meters, active-step selection, and the
  inspector's selected-step count.
- Presets sidebar and Acid Squelch selection, with changed panel values.
- Adding Str8 Delay from Audio FX and arp from MIDI FX by double-click; the
  respective panels appeared in the chain.
- A synth cutoff p-lock marker, its right-click Clear p-locks action, and removal
  of the marker after clearing. Knob gestures during Record + Play subsequently
  produced cutoff/resonance locks.
- Track arming and computer-keyboard note recording; notes appeared in the grid
  and piano roll, including a note in the higher octave.
- Double-clicking the mixer name badge opened the piano roll. The Lane menu
  initially held step parameters; after recording device locks it included
  `inst lp_freq` and `inst lp_res`. Editing the cutoff lane changed its points.
- Arrangement view, Set starting scene, Place followed by a click in a track
  lane, and the resulting pattern clip.
- Arrangement recording produced a labeled Take 1 clip after Stop. The source
  panel showed Take 1 and Loop off, separately from the earlier pattern clip.
- Instrument Rack activation created a Layer Rack with the instrument/sample
  drop area. Double-clicking a synth while it was selected replaced the rack,
  confirming why the manual distinguishes activation from a layer drop.
- Scene + created a second scene and additional track pattern cells. Bus A
  selection displayed its reverb chain.
- File > Save As saved `Manual-Walkthrough-2026-09-09.json`; file existence was
  confirmed under the installed app's Application Support projects directory.
  This is an intermediate snapshot. Later exploratory changes were not saved
  before the machine locked; the stopped app retains that later working state.

These observations verify visible behavior, not subjective audio quality. Meter
activity is not claimed as an acoustic listening test. Computer-use screenshots
are in the task history, not embedded in the manual's deliberately small format.

## Remaining source-supported gesture review

After the unlocked follow-up below, `eseq-ug3m.8` retains successful factory/sample
browser-tree drops to drum pads and new tracks, multi-track modifier-click grouping,
layer-FX drops, drum-rack/member recording, kit save/recall, sample import, and
individual pattern-launch capture. Rack macro assignment is verified; continuous
macro movement and range/curve editing still need gesture review. The computer-use
API did not reliably complete every drag. Its documented click API also does not
provide a modifier-click option, so modifier-based multi-selection was not claimed
as exercised. These limitations do not establish app defects.

## Unlocked hands-on follow-up

The user unlocked the Mac and requested continued work. This pass verified:

- A Digi Drift engine drop populated the empty Layer Rack. A subsequent Digi Wave
  factory-item drop added a second layer; both rows and the selected layer's Slot FX
  area were visible. A loaded Digi Wave engine dropped into the mixer's empty area
  created a fifth independent track. Some other browser-tree drags did not complete.
- Drum Rack activation created its pad container. Selecting its header showed the
  C1-based pad grid and shared chain. Double-clicking Glue Compressor in Audio FX
  added it alongside that rack's pads. No successful pad population is claimed.
- Record + Play begun in arrangement view, followed by Scene 1 and Scene 2 launches,
  produced distinct scene spans and corresponding track-pattern clips after Stop.
- The rack's macro dial toggle exposed eight macros. Map 0 opened the mapping
  sidebar and green eligible controls. Clicking Digi Wave layer cutoff created a
  `Layer 2 · Synths/Cutoff` mapping, displayed 20–18000 Hz limits and live state,
  and changed the button to map 1. Done returned to the browser. CUA drag/scroll
  attempts did not establish a continuous macro sweep, so that is still unverified.
- Digi Wave's save icon opened Save Preset. Save as New with the name
  `Manual Walkthrough Wave` succeeded; that entry appeared in Presets afterward.
- File > Save As saved `Manual-Walkthrough-Verified-2026-09-09.json`. A later
  File > Save preserved the macro and shared-rack-effect edits too. The app was
  left stopped with both Record and WAV off.
- File > Export Audio, Beat range 0–8, 48000 Hz and a two-second tail completed.
  The output `Manual-Walkthrough-Verified-2026-09-09 (1).wav` has a valid stereo
  float WAV header and 2,304,000 audio bytes, matching six seconds at 48 kHz.
- WAV capture stayed visibly active after transport Stop. Turning WAV off wrote
  `recording-1788986457.wav`, a stereo 48 kHz WAV with 3,555,328 audio bytes.

Artifacts remain in the installed app's Application Support projects/recordings
folders. They are walkthrough evidence, not polished demonstration music. Neither
export nor WAV capture received an acoustic listening assessment.

## Second editorial pass

The user requested continued improvement after the first draft. With the Mac
still locked, the follow-up used source handlers and headless manual rendering.
It corrected the first-session deselection gesture: selecting the same track
does not necessarily clear steps; Command-click toggles individual selections,
and switching tracks clears them. It expanded layer controls, rack macro mapping,
drum-pad versus chromatic member arming, kit saving, effect insertion/move scopes,
preset overwrite targeting, sample import tagging, and an eight-bar arrangement
exercise. Saving and audio export is an additional everyday-workflow chapter.

The current checkout's `export-song-start` calls `capture_export_project`, which
captures current authoring state. Its dialog still says it exports the last saved
version. These are concurrent application changes outside this documentation task;
the manual instructs readers to save before exporting and does not assert that
stale label as the implementation contract. Unsaved export-state semantics and the remaining rack/kit gestures join human review.
The later unlocked pass exercised saved-project export and master WAV capture, as recorded above; it did not test unsaved export-state semantics.

## Main source anchors

- `content/ui/browser.lisp`: activation versus new-track drops, sample preview,
  preset selection and saving; `content/ui/effects/instrument-panel.lisp`: rack
  slots, layer versus track effects, preset save control.
- `content/ui/step-grid-interactions.lisp` and `sequencer-keys.lisp`: empty-step
  entry, active-step selection, double-click removal, ranges and cursor behavior.
- `crates/sequencer/src/ui/input.rs`: musical typing, octave keys, focus/arming,
  grouping, view switching, and context-sensitive duplication.
- `content/ui/effects/param-controls.lisp` and
  `crates/sequencer/src/ui/host_commands/instrument_params.rs`: lock menus and
  Record + Play gesture printing with no selected steps.
- `content/ui/piano-roll.lisp`, `crates/sequencer/src/ui/piano_roll.rs`, and
  `crates/eseqlisp/src/widget_render/automation_lane.rs`: device menu entries only
  for locked parameters, gray base values, colored locks, and double-click clear.
- `content/ui/mixer.lisp`: multi-selection, grouping, mute/solo/arm, routing and
  track pattern cells; `content/ui/transport.lisp`: scene duplication and controls.
- `content/ui/arrangement.lisp`, `crates/eseqlisp/src/widget_render/timeline.rs`,
  and `crates/sequencer/src/app/song_transport.rs`: placement, title-bar opening,
  independent scene markers, capture and playback authority. The current
  unified-transport and arrangement-lane-model specs helped interpret the code;
  older SONG/SESSION-mode instructions were intentionally not reused.
- `crates/sequencer/src/ui/natives.rs`: selected-step toggling, track-switch
  selection clearing, drum-rack arming, and master WAV recording.
- `content/ui/sample-import.lisp`: staged tree, batch/subtree/file tagging.
- `content/ui/export-song.lisp`, `crates/sequencer/src/ui/host_commands/export.rs`,
  and `crates/sequencer/src/app/projects.rs`: export controls and capture scope.
- `content/midi-fx/*/dsp.lisp`: built-in MIDI-effect parameters and transformations.

## Validation

The markdown audit found 18 pages, 51 valid internal links, one H1 per page,
no unreachable chapters, and no tables/images/HTML/nested lists in the authored
content. Generated `ref-*` pages are not linked while their generator remains
unimplemented (`eseq-ug3m.4`), avoiding dead introductory navigation.

The existing renderer/navigation tests had hard-coded placeholder words and
chapter order. Those examples now live in `crates/sequencer/tests/fixtures/manual/`
and are loaded by a test-only page provider. The tests retain their formatting,
click, action, and navigation contracts without constraining the real prose.
A separate test opens every actual chapter, checks the loaded title against its
source, and asserts finite/nonzero content and heading geometry.

Passed:

```sh
cargo nextest run -p sequencer --bin metal_seq -E 'test(/manual_ui_tests/)'
# 5 passed; 875 unrelated tests skipped.

cargo nextest run -p sequencer --bin metal_seq -E 'test(=state_values::tests::manual_ui_tests::manual_authored_pages_have_visible_content)'
# 1 passed after strengthening the authored-content/title check.
# Passed again with all 18 chapters after the second editorial pass.
```

An initial test-fixture implementation used inline source strings and failed
four fixture tests. It was corrected to read the fixed Markdown files through
the real parse-manual-page path; the rerun above passed. No failure is being
reported as an unrelated or pre-existing baseline problem.

Headless Metal captures were opened and inspected:

- `/tmp/eseq-manual-index.png`: index hierarchy, flowing prose, chapter links.
- `/tmp/eseq-manual-parameter-locks.png`: headings, numbered instructions,
  paragraphs, and scrollable chapter content.
- `/tmp/eseq-manual-racks.png`: expanded rack introduction and layer workflow.
- `/tmp/eseq-manual-saving-and-export.png`: new save/export chapter and lists.

These used `metal_seq capture --buffer '*manual*' --width 1200 --height 900`.
The index uses the checked-in manual capture fixture. The p-lock page uses a
scratch capture fixture that opens that node after synchronization. No renderer
changes were needed. Screenshots are local QA outputs, not packaged manual assets.

No application behavior changed, no package-wide formatting or exhaustive test
suite ran, and no commit, push, or Dolt remote sync was performed. The remaining
uncertainty is workflow verification listed above, not a known code workaround.
