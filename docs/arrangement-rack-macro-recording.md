# Arrangement take recording: rack macros

While recording an arrangement take on an armed Instrument Rack, UI macro
controls and MIDI mappings to `rack-macro` use the same capture path. The
armed target, not UI focus or a selected pattern step, determines the lane.

A touched macro latches its latest position for the rest of that take. This
is distinct from pattern view's pointer hold-to-print gesture: ordinary MIDI
CC controllers do not supply a release event. Untouched macros retain their
sound's base positions. No source pattern or shared sound default is edited.

## Timing and storage

Controller changes are staged in the take session using the compensated
record clock, independently of note release. Thus turning a knob during the
first held chord works even though the note recorder has not created its
pending lane yet. The existing take rule still applies: a performed note
punches in the take; controller input alone does not create a new note clip.

At commit, the recorded controller stream is materialized as macro p-locks
on the take's step grid. A change lands on the containing step. A macro's
latest value is carried through subsequent steps, including empty steps and
chunk boundaries, until the take ends. A position set before the first note
is stamped at punch-in. This uses the same beat offset/timebase as the notes,
not the looping scene pattern's playhead. As with p-locks generally, playback
resolution is one value per step, not sample-accurate continuous automation.

Macro p-locks belong to `TrackPatternSeq`. The shared `Patch` retains macro
names, mappings, and base positions only. Composing a working rack snapshot
reunites the two. This allows separate takes and chunks to share one sound
without sharing their automation. Project serialization continues using the
composed representation, so the on-disk macro format is unchanged.

Preflighted arrangement rows/chunks retain their own macro locks and authored
positions. Preparing a later row must not overwrite earlier rows through the
live pattern's scalar cache. Explicit knob edits after preflight remain live
through per-macro edit revisions; row synchronization/publication is not a
controller edit, and stored macro p-locks retain precedence over base edits.

## Monitoring and lifecycle

A separate per-track atomic override holds the current take macro values.
The audio callback consumes its changed-track mailbox and updates only the
mapped targets on that rack's sounding voices. It does not apply source
pattern p-locks. Live note-ons read the same overrides, so new notes and held
notes agree. This also works on empty arrangement lanes, where there are no
sequence events to drive a pattern-print latch.

Controller monitoring responds at the next audio block; recorded playback
uses the step-quantized values described above. A normal pointer release or
focus change cannot disarm hardware capture. Stop, punch-out, Cancel, and a
new capture clear the override. Commit/undo/redo use the existing take
transaction. Failed commits retain the pending performance but clear live
monitor overrides.

Binding refreshes must not push raw instrument defaults over sounding rack
voices. Instrument-mode custom slots and samplers are stamped at note-on,
just like non-rack tracks; free-running patches keep their defaults push.

This path is for rack macros. It does not add automation-only clips or
change the recording behavior of other device-control families.
