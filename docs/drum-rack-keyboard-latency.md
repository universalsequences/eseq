# Drum-rack keyboard latency — 2026-09-10

The reported reproduction is computer-keyboard playing in the saved
`drumandbass` project with its drum rack armed. Hardware MIDI does not show
the same flam. The inspected project has seven custom drum instruments,
157 BPM, three scenes, and no MIDI FX on the drum tracks. Its SHA-256 is
`e8f87839be096abf58a8dfe33c4f579f391a9af854622dbd2941c38781729763`.

## Cause and change

`metal_seq` previously removed **one** backend event per outer iteration.
Control work, reactive synchronization, frame construction and presentation
could run before the next keyboard event, even if that event was already
queued. Hardware MIDI already drained its queued events together. Both
sources subsequently use the same live-note router and audio callback.

The keyboard loop now drains consecutive consumed live-key events with a
zero poll timeout. Each note still uses the existing mode/focus gate, rack
pad resolution, recording, roll and note-off handling. An ordinary editor
event or a pending host command ends the batch so commands retain their
ordering. The batch yields after 64 events or 4 ms of handling work; the
initial blocking poll is outside that budget. This is a work limit, not a
delay added to the notes.

The reactive synchronization function is now separate from presentation.
The production loop and the headless latency probe call the same function.
No DSP, audio buffer size, MIDI routing, or recording quantization changed.

## Results

Apple M1 Max, release profile, working tree based on `7c820bbc`. The full
reactive-tick probe ran alone after compilation, with five warmups and 25
measured pairs per case (50 individual key presses/releases per case):

| Measurement | Previous scheduling | Batched scheduling |
| --- | ---: | ---: |
| Queued pair, median | 577.875 µs | 26.791 µs |
| Queued pair, p95 | 708.209 µs | 33.042 µs |
| Queued pair, maximum | 841.250 µs | 41.958 µs |
| Individual live gate/dispatch, median | 14.083 µs | 14.042 µs |
| Recorded release, median | 334.500 µs | 339.916 µs |

The median queued-pair interval improved **21.57×**, and p95 improved
**21.43×**. The improvement removes UI work between notes; it does not
claim a faster instrument or a faster individual live-note gate.

Ten targeted UI tests/probes passed: rack pad/base-pitch routing, per-pad
recording quantization, held releases across mode changes, numeric focus,
mode opt-in, sequence roll, MIDI parity, both batch ordering/budget tests,
and the saved-project timing probe. The separate saved-instrument audio
probe also passed: **11 checks in total**. No full package/workspace suite
was run. The saved project hash was unchanged after both probes. The
audible result has not been verified with physical typing; complete native
event-to-audio measurement is tracked in `eseq-77bt`. This is a remaining
measurement limit, not evidence of a 22× reduction in acoustic latency.

## Measurement boundaries

The UI probe loads the saved project and production multi-pane UI, arms
the rack, starts recording/playback in isolated state, and plays `a`/`w`
(the first two drum pads). It measures dispatch to the live-trigger
channel. The reference case interleaves a UI update between the two
already-queued notes, as the old event loop could do; the new case uses
the production batch policy. Both cases exercise actual recording on
release. It advances playheads and runs real reactive synchronization and
frame construction, without an artificial sleep.

These timings describe **queued-note dispatch across a UI update**. They
are not physical keyboard-to-speaker latency. They exclude OS delivery,
the router thread hop, audio-buffer waiting, DAC latency, GPU presentation,
and the other outer-loop services. A key arriving during an already-running
UI update can still wait for that update. The change removes interleaved
frame work from a queued burst; it does not make the UI thread realtime.

A separate audio probe loads the same saved instruments, queues two notes
and calls the production audio callback at 48 kHz / 512 frames. It checks
both render timestamps, voice ownership, finite output and audible signal.
For 25 pairs, both notes had **zero samples of render-timestamp skew**.
Queueing the pair and rendering the complete block took median **1.028 ms**,
p95 **1.186 ms**, maximum **1.208 ms**; output peak was **0.4563**.
This excludes waiting for a device callback. A 512-frame block at 48 kHz
lasts 10.67 ms regardless of the keyboard dispatch improvement.

An earlier, narrower probe measured frame construction/playhead publication
without the full reactive tick: queued-pair median **234.792 µs** versus
**15.125 µs** batched (15.52×), and live dispatch around **8 µs**. Those
numbers alone do not explain an audible flam.

## Reproduce

Use a quiet machine and run the probes separately, in release mode. The
saved project is local user data and is deliberately not checked into git.
Neither probe saves changes to it or opens an audio device.

```sh
ESEQ_LIVE_INPUT_PROJECT=/absolute/path/to/drumandbass.json \
  cargo nextest run --release -p sequencer --bin metal_seq \
  -E 'test(=tests::drum_rack_keyboard_dispatch_latency)' \
  --run-ignored only --no-capture

ESEQ_LIVE_INPUT_PROJECT=/absolute/path/to/drumandbass.json \
  cargo nextest run --release -p sequencer --lib \
  -E 'test(=audio::live_input_tests::saved_drum_rack_live_notes_share_a_render_sample)' \
  --run-ignored only --no-capture
```
