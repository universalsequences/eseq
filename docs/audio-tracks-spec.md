# Audio Tracks — spec

Status: draft (rev 1, 2026-10-17). Epic: see §13 for the bead slices.

Every track today is a software instrument (or a modulator) driven by
patterns, scenes and takes. This spec adds **audio tracks**: recorded or
imported audio laid out as regions on the arrangement timeline — the classic
"waveforms on a timeline" model — so a guitar part can be recorded against the
sequenced parts and a whole song finished inside eseq.

The first milestone is deliberately narrow: **record a guitar take to a WAV,
aligned to the timeline, and hear it back.** Everything else (comping,
warping, session clips, cross-clock devices) is phased after it and must not
be painted out by it.

## 1. Summary of decisions

1. **An audio track is a track kind**, not a new top-level concept. Its source
   is a timeline region player instead of an instrument, so it inherits the
   mixer channel, fx chain, sends, groups, solo/mute and PDC unchanged.
2. **Non-destructive by default.** Audio lives in immutable *source assets*;
   a *region* references a span of one. Edits change regions only.
   "Destructive" operations (reverse, normalize, consolidate, bounce) render
   a **new** source asset and repoint the region — destructive-feeling, fully
   undoable, nothing lost.
3. **Audio regions bypass the row compile.** The arrangement compiles lanes to
   `ProjectSong` rows on the step grid; audio needs sample-accurate placement,
   mid-region starts and crossfades. Audio lanes are read by the audio thread
   as a region list mapped through the transport's beat→sample position.
4. **Arrangement-only in v1.** Audio tracks have no scene/session clips in v1.
5. **No audiograph (C) change.** Input and region playback are Rust
   `NodeVTable` nodes, the same way `audio_tap.rs` and the live analyzer add
   nodes today.
6. **v1 requires input and output on one clock** (one interface, or an
   Aggregate Device). Mismatches are *detected and explained*, not corrected.
   Cross-clock support slots in later behind the `InputSource` boundary (§6.3)
   without touching anything else.
7. **Recorded latency is round-trip, calibrated by loopback.** The existing
   plugin-delay compensation stays and is a separate term (§7).

## 2. Current facts this spec builds on (verified 2026-10-17)

- Audio output is a single cpal output stream (`src/audio/stream.rs`,
  `build_output_stream`) driving the vendored C audiograph
  (`crates/sequencer/audiograph/`, built in `build.rs`) in fixed graph blocks
  (`FixedOutputBlocks`). **There is no audio input stream anywhere** in the
  crate; `src/recorder.rs` (`MasterRecorder`) captures the master *output*
  only.
- Rust-defined graph nodes are an established pattern: `audio_tap.rs:353`
  (`audio_tap_vtable`), `ui/live_audio_analyzer`, and the impulse/constant
  test nodes in `app/graph/latency.rs:712` and `app/graph/tests.rs:143`.
- `app/graph/latency.rs` is **plugin delay compensation**: effects declare
  `latency_samples` (e.g. Filter Table, `effects/filter_table.rs:73`), serial
  chains sum, parallel joins get `pdc_delay` pads. It aligns signals *inside*
  the graph; it knows nothing about device input latency.
- Record latency for note/take recording is
  `SequencerState::total_record_latency_seconds`
  (`sequencer/state/sequencer_state/transport.rs:161`) = a device term set in
  `stream.rs` to *one output block* (CPAL exposes no portable latency) plus
  the planner's graph term. That device term was designed for MIDI, where the
  input event is instantaneous; audio input adds its own latency on top.
- Track kinds: `ProjectTrackKind` (`project.rs:2091`) / `InstrumentType`
  (`sequencer/data.rs:196`) = Empty, Sampler, Custom, Modulator, Rack.
- Arrangement: `ProjectSong` rows (`sequencer/state/song.rs:139`) on beats,
  per-track `LaneSource` = `Pattern | Take | Empty` (`song.rs:83`). Takes are
  step-domain note captures (`app/take_recording.rs`).
- `hound` (WAV write) and `symphonia` (wav/aiff/flac/mp3 decode) are already
  dependencies.
- RT hazard on record: the scheduler-snapshot seam has dropped large `Arc`s on
  the audio thread before (bead eseq-sj01). Region lists and source buffers
  must never be freed on the audio thread.

## 3. Terminology

- **Source asset** — an immutable audio file owned by the project, identified
  by `AudioSourceId`. Never modified after it is written.
- **Region** — a placement of a span of one source on an audio track's lane.
- **Take** (audio) — one recording pass; produces one source asset and one
  region. Multiple passes over a span are a take stack (comping, phase 2).
- **Input source** — the boundary between a device input stream and the
  graph (§6.3).
- **Round-trip latency** — time from a sample leaving the graph to the
  same sound arriving back as recorded input, through the device.

## 4. Goals

- Record mono or stereo input from a chosen device/channel onto an armed
  audio track, in time with the sequenced parts.
- Monitor the input live through the track's fx chain (amp sims, DGen fx).
- Import audio files as regions.
- Move, trim, split, fade, gain, duplicate and delete regions with undo.
- Save/load projects with audio tracks; sources live with the project.

## 5. Non-goals (v1)

- Warping / time-stretch / tempo-map following (phase 3).
- Audio clips in session scenes (phase 3).
- Cross-clock input (USB mic + headphones) — detected, not handled (phase 2,
  §6.4).
- Disk streaming — v1 loads sources into memory (phase 2).
- Comping UI — v1 keeps the latest take on top (phase 2).
- Direct (hardware) monitoring control beyond an on/off toggle.

## 6. Input path

### 6.1 Shape

```
cpal input callback ──► input ring (SPSC, per device)
                              │
                      InputSource (§6.3)
                              │
          input node (Rust NodeVTable, 0 inlets, 1–2 outlets)
             │                               │
     armed track fx chain             record tap ──► writer ring ──► writer thread ──► WAV
     (monitoring, if on)              (raw, pre-fx)
```

- The **input node** runs inside the graph callback, so every block it reads
  is stamped with the *output* transport's sample position. That is what makes
  timeline alignment exact (§8.2); recording in the cpal input callback would
  lose that relation.
- The **record tap is pre-fx**: the WAV holds the dry DI signal so amp-sim
  choices stay editable after recording. (Printing fx is a later option.)
- The **writer thread** drains the writer ring into `hound`. The audio thread
  never touches files, never locks, never allocates; a full writer ring
  counts a dropped block (same contract as `MasterRecorder`'s `try_lock`)
  and is surfaced to the user, never silently ignored.
- One input stream per open input device, shared by every armed track that
  selects a channel on it.

### 6.2 Device selection

- Input device + channel(s) are explicit and **saved with the project**
  (per audio track: device id, channel or channel pair). Never default to
  the system default input: on the dev machine that is BlackHole, a virtual
  loopback device, which would record silence.
- Device list comes from cpal; channel count from the device's input config.

### 6.3 The `InputSource` boundary

The input node reads through one trait-shaped seam:

```
read(block: &mut [f32], frames) -> InputRead { frames_read, underrun }
latency_frames() -> u32      // fixed lag this source adds (feeds §7)
```

v1 implementation: **plain ring with a fixed safety margin.** The node starts
reading `margin` frames behind the writer so input/output callback jitter
never empties it. The margin is a latency term.

### 6.4 Clocks and rates

cpal has no duplex stream, so even one device yields two callbacks. With a
shared clock that is fine; across clocks the ring drifts (tens of ppm → a
click every few minutes) and a rate mismatch empties it almost immediately.

On arm, check:

1. **Rate.** If the input device's nominal rate ≠ the graph rate, set the
   input device's nominal rate to match (CoreAudio
   `kAudioDevicePropertyNominalSampleRate`). If that fails, refuse to arm and
   say why. (Dev machine today: MacBook Pro Microphone 44.1 kHz vs Speakers
   48 kHz.)
2. **Clock.** Same device → OK. Different devices → compare
   `kAudioDevicePropertyClockDomain`; equal and nonzero → OK. Otherwise refuse
   to arm with "input and output run on different clocks — use one
   interface, or create an Aggregate Device with drift correction in Audio
   MIDI Setup."

**Cross-clock follow-up (phase 2):** an adaptive resampler inside
`InputSource` watches ring fill and nudges its ratio to hold a target fill
(the Aggregate Device / DAW approach). A rate mismatch is the same mechanism
with a base ratio ≠ 1.0. Nothing outside `InputSource` changes; its
resampler delay joins `latency_frames()`.

Linux: CPAL/ALSA/PipeWire equivalents are not designed here; v1 ships the
same-device rule there with the clock check reduced to "same device".

### 6.5 Monitoring

- Per audio track: **arm** and **monitor** (`off | auto | on`; auto = on
  while armed and not playing back a region at that point).
- Monitoring routes the input node into the track fx chain → mixer.
- When the input and output are both built-in devices, monitor defaults to
  **off** (mic + speakers feed back).
- Latent fx on an armed track delay monitoring by their PDC. v1 accepts this
  and shows the monitoring latency in the track header; a "low-latency
  monitoring" bypass is an open question (§14).

### 6.6 macOS permission

Without `NSMicrophoneUsageDescription` and TCC approval, the input stream
opens successfully and delivers **silence**. The app bundle needs the usage
string; unbundled dev runs get the prompt on behalf of the terminal. Detect
an all-zero input after arm and point at System Settings › Privacy ›
Microphone rather than recording silence silently.

## 7. Latency

Two separate problems, both needed:

| Term | What | Owner |
|---|---|---|
| Graph PDC | latent fx padding inside the graph | existing `app/graph/latency.rs` |
| Output device | graph → speaker | today: one block (estimate) |
| Input device | mic/DI → input callback | new |
| Input ring margin | `InputSource::latency_frames()` | new |

A recorded sample stamped at transport position `t` was *played against*
audio the performer heard `output_latency` earlier and reached the graph
`input_latency + margin` later. The region offset is

```
recorded_pos = stamped_pos − (output_device + input_device + margin)
```

Graph PDC is **not** subtracted: the recorded audio enters the graph at the
track and is compensated on playback by the same planner as every other
track.

**Calibration (v1):** a loopback ping. Play an impulse out (borrow the
impulse node from `latency.rs` tests), record the input with the user's cable
or mic-near-speaker, cross-correlate, store the measured round-trip per
(input device, output device, rate, block size). The measured value replaces
the output + input device terms; margin is still added from `InputSource`.
Uncalibrated fallback: output block + input block + margin, plus a manual
offset field (ms) in the transport/device settings — same spirit as the
existing tunable record-latency value.

The note/take record path keeps using `total_record_latency_seconds`
unchanged; audio recording gets its own total, because MIDI input has no
input-device term.

## 8. Data model

### 8.1 Types

```
AudioSource {               // immutable, project-owned
    id: AudioSourceId,
    file: relative path,     // §8.4
    sample_rate, channels, frames,
    peaks: cached overview (regenerable)
}

AudioRegion {
    id: AudioRegionId,
    source: AudioSourceId,
    start_beat: f64,         // timeline position (arrangement is beat-based)
    source_offset: u64,      // frames into the source
    length: u64,             // frames
    gain_db: f32,
    fade_in, fade_out: u32,  // frames, equal-power default
    muted: bool,
    take: Option<AudioTakeId>  // groups passes for comping (phase 2)
}

ProjectTrackKind::Audio { input: Option<AudioInputSelection>, regions: Vec<AudioRegion> }
InstrumentType::Audio
```

- Positions on the timeline are **beats**, like everything else in the
  arrangement; content is **frames**. Without warping, a tempo change moves a
  region's start but not its content speed (Ableton "warp off" semantics).
- Overlaps on one track: the later region (in z-order) wins; the overlap
  edge gets the shorter region's fade. No implicit mixing of overlaps.

### 8.2 Playback

- One **region player** node per audio track (Rust `NodeVTable`), feeding
  the track fx chain like an instrument voice sum would.
- The audio thread reads an immutable `Arc<AudioTrackPlayback>` (sorted
  regions + `Arc<[f32]>` source buffers) published by the control thread.
  Old snapshots are retired to a control-thread drop queue — **never dropped
  on the audio thread** (eseq-sj01).
- Per block: map the block's transport sample range → beats → active
  regions; read with fades/gain; seek is just "which offset at this sample",
  so starting playback mid-region is free.
- Loop and region boundaries apply short declicking fades.
- Bounce/export (`app/graph/bounce_latency.rs` etc.) must render audio
  tracks through the same player.

### 8.3 Relation to rows, takes and scenes

Audio lanes are **not** compiled into `ProjectSong` rows and have no
`LaneSource`. Scene launches and session view ignore audio tracks in v1
(they play their arrangement regions when the arrangement plays; silent in
session playback). Audio takes are a separate `AudioTakeId` space from note
`TakeId`s; the comping *UI* can share concepts with the takes work, not
storage.

### 8.4 Storage

- Sources are written as 32-bit float WAV via `hound` into a per-project
  audio folder (open question §14 on exact location/consolidation).
- Project file stores sources by relative path + content hash; missing
  sources load as offline regions (drawn, silent, relinkable), never an
  error that blocks project load.
- Serialization bumps the project version; older files load with no audio
  tracks.

## 9. Recording semantics

- Arm one or more audio tracks; transport Record starts all armed inputs.
- On start, the input node records the transport sample position of the
  first recorded block; on stop the writer finalizes the WAV and the control
  thread creates `AudioSource` + `AudioRegion` with `start_beat` from the
  latency-corrected position (§7).
- Recording over an existing region: v1 places the new region on top
  (non-destructive; the old one is still underneath and restorable by
  deleting/trimming the new one). Phase 2 turns this into a take stack.
- Punch in/out (phase 2) = only keep the recorded span inside the punch
  range; the WAV can still capture pre-roll for later trim extension.
- Loop recording (phase 2) = one source, sliced per loop pass into take
  regions.
- Undo of a recording removes the region; the source file is kept until the
  project is saved without references to it (garbage collection on save,
  §14).

## 10. Editing

Non-destructive (region-only, undo via the existing history host-command
pattern): move, trim either edge, split, duplicate, delete, gain, fades,
mute, nudge, slip (move content under fixed edges = change `source_offset`).

Render-to-new-source (still undoable): reverse, normalize, consolidate
(merge regions → one source), bounce-in-place (print fx), and later
time-stretch. The old source stays referenced by history until GC.

## 11. UI (sketch)

- Arrangement: audio track rows draw regions with a waveform from cached
  peaks (`timeline` widget content kind, like the rack-lane `Rings` content).
- Track header: input selector, arm, monitor, input meter, monitoring
  latency readout.
- Settings: input device, calibration ("Measure latency…"), manual offset.
- Add Track menu gains **Audio**.

## 12. Phasing

**Phase 1 — record a take (the first milestone)**
1. Input stream + device/channel selection + input ring + `InputSource`
   (plain) + input node + input meter.
2. Rate/clock/permission checks on arm (§6.4, §6.6).
3. Audio track kind, model types, serialization, region player (in-memory),
   arrangement rows drawing regions with waveforms.
4. Arm / monitor / record tap / writer thread / region creation with
   latency-corrected stamping.
5. Loopback latency calibration + manual offset.
6. Import audio file as region.
7. Region editing: move/trim/split/fade/gain/duplicate/delete + undo.
8. Bounce/export includes audio tracks.

**Phase 2**
- Cross-clock and rate-mismatched input via adaptive resampler in
  `InputSource` (USB mic + headphones).
- Loop recording, punch in/out, take stacks + comping UI.
- Render-to-new-source ops; source GC on save.
- Disk streaming for long sources.

**Phase 3**
- Warp markers / time-stretch (share sampler stretch DSP), tempo following.
- Audio clips in session scenes.

## 13. Beads

Epic `eseq-audio`; slice titles mirror §12.

| Bead | Slice | Depends on |
|---|---|---|
| eseq-audio.1 | input stream, device/channel, `InputSource`, input node, meter | — |
| eseq-audio.2 | arm-time rate/clock/permission checks | .1 |
| eseq-audio.3 | track kind, model, serialization, region player, waveform rows | .1 |
| eseq-audio.4 | arm/monitor, record tap, writer, stamped region creation | .1, .3 |
| eseq-audio.5 | loopback calibration + manual offset | .4 |
| eseq-audio.6 | import audio file as region | .3 |
| eseq-audio.7 | region editing + undo | .3 |
| eseq-audio.8 | bounce/export renders audio tracks | .3 |
| eseq-audio.9 | cross-clock input (adaptive resampler) — P3 | .1 |
| eseq-audio.10 | loop/punch recording, take stacks, comping — P3 | .4 |
| eseq-audio.11 | render-to-new-source ops + source GC — P3 | .4 |
| eseq-audio.12 | disk streaming — P3 | .4 |
| eseq-audio.13 | warping + session clips — P3 | .4 |

## 14. Open questions

1. **Source location**: per-project `<project>.audio/` folder next to the
   project file vs content-addressed store in the user data root with
   collect-on-export? (Leaning: per-project folder, simplest to back up.)
2. **Low-latency monitoring**: bypass PDC for armed+monitored tracks (and
   accept misalignment while monitoring) as Ableton's "reduced latency when
   monitoring" does?
3. **Print fx while recording**: optional second tap post-fx?
4. **Source GC**: delete unreferenced sources on save, or move to a trash
   folder for one session?
5. **Rack membership**: can an audio track be a rack member? (Leaning no in
   v1.)
6. **Linux**: CPAL/PipeWire input latency and clock detection.
