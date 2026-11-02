# Audio Tracks — spec

Status: rev 2.1, 2026-11-01 — phase 1 built (uncommitted); "as built" notes
inline where the implementation settled a choice differently from rev 2.
Epic: see §15 for the bead slices.

Every track today is a software instrument (or a modulator) driven by
patterns, scenes and takes. This spec adds **audio tracks**: audio laid out as
regions on the arrangement timeline — the classic "waveforms on a timeline"
model — first by dragging samples in, later by recording.

**Rev 2 reorders the work.** The first milestone is no longer recording; it
is **Ableton parity for placed audio**: drag one or more samples from the
browser onto an audio track, see them as waveform regions, edit them on the
timeline (move, trim, copy/paste, duplicate, delete), double-click one to
open a waveform clip editor, and hear the result in playback and bounce.
That milestone builds the whole model — track kind, regions, player,
storage, editing, editor — with none of recording's hardware problems
(input streams, clocks, permissions, latency). Recording (rev 1's milestone)
becomes phase 2 and creates regions through the same model. Rev 1's input
and latency design is kept intact in §12–§13.

## 1. Summary of decisions

1. **An audio track is a track kind**, not a new top-level concept. Its source
   is a timeline region player instead of an instrument, so it inherits the
   mixer channel, fx chain, sends, groups, solo/mute and PDC unchanged.
2. **Sample playback is shared with the sampler.** The sampler's decode,
   buffer, rate-conversion and interpolation code moves into a shared sample
   core (§6) used by both. A 44.1 kHz file plays back identically in a
   sampler and on an audio track. The sampler's richer features (speed, SR
   reduction, scrub, warp, mod lanes) are later exposed on audio tracks
   through the same core (§6.3) — not reimplemented.
3. **Dropping a sample on an existing audio track creates a region** at the
   drop position. Dropping on the new-track zone keeps today's behavior
   (§7.1).
4. **Sources are copied, never referenced in place.** A dropped sample is
   copied into the content-addressed audio store; the project never points
   into the library (§9.4).
5. **Non-destructive by default.** Audio lives in immutable *source assets*;
   a *region* references a span of one. Edits change regions only.
6. **Double-click a region → waveform clip editor** in the lower panel, the
   way a synth clip opens the piano roll (§8). Start/end markers there set
   the region's source window; warp markers land there later.
7. **The FX panel works unchanged** for audio tracks: the same gesture opens
   the same lower-panel fx chain. v1 shows **no device** slot (§8.4).
8. **Audio regions bypass the row compile.** Audio lanes are read by the
   audio thread as a region list mapped through the transport's beat→sample
   position, not compiled into `ProjectSong` rows.
9. **Arrangement-only in v1.** No scene/session clips.
10. **No audiograph (C) change.** The region player is a Rust `NodeVTable`
    node. *As built:* it reads reference-counted PCM held by its playback
    snapshot rather than graph buffers, so it never depends on when a
    `create_buffer` edit lands on the audio thread.

## 2. Current facts this spec builds on (verified 2026-11-01)

Sampler (`crates/sequencer/src/instruments/sampler.rs`, 3.1k lines):

- `load_wav_buffer` (`:1739`) decodes via `eseqlisp::audio::sample::load_wav_file`,
  folds to stereo (mono duplicated, >2 channels averaged per side), **trims
  leading silence** (64-frame RMS windows, −60 dB), and registers the result
  in the graph buffer pool with `create_buffer` (`audio/audiograph.rs:229`).
  The trim is a sampler policy that would break timeline alignment; the
  shared core must not apply it (§6.1).
- Sources stay at their native rate. Playback steps by
  `sampler_playback_step` (`:344`) = `rate × source_sr / host_sr`, reading
  with linear interpolation (`read_interpolated`, `:350`). That is the
  "rate conversion" the sampler does today, and what audio tracks adopt.
- The sampler voice is a Rust `NodeVTable` (`sampler_vtable`, `:1726`) with a
  121-slot state block: start/end, reverse, loop modes, SR reduction, scrub
  (with vactrol/hand model), **warp** (re-pitch and Beats modes with onset
  tables, `STATE_WARP_*`), and four modulation lanes per param fed by node
  inputs (`STATE_MOD_*_LANE{1..4}`, `modulation_input` `:383`).
- Library samples live in a content-addressed store
  (`AppPaths::samples_dir`, `resolve_sample_ref` `app_paths/mod.rs:646`);
  projects reference sampler samples as `samples/<sha256>.wav`
  (`load_project_sample_asset`, `app/projects.rs`).

UI:

- `waveform` widget, used by the sampler panel
  (`content/ui/effects/sampler-panel.lisp:451`): binds a graph `:buffer`,
  view window, seconds ruler, playhead, start/end selection handles, slices,
  `:on-action` events. Reusable for the clip editor as-is.
- Arrangement lanes are `timeline` widgets with `:drop-types` / `:on-drop`
  (`content/ui/arrangement.lisp:1971`), clip double-click →
  `seq-open-arrangement-piano-roll-bottom-for-track` (`:1510`, `:1539`), and
  region `:copy-items` / `:paste-items` → `seq-song-region-copy/paste`
  (`:1216`, `:1575`). The new-track drop zone (`:2643`) accepts
  `"sample" "instrument" "instrument-preset" "sound"` and calls
  `eseq.sequencer/drop-new-track`.
- Lower panel buffers: `*fx*` and `*piano-roll*`, switched in
  `content/ui/seq-panels.lisp` (`seq-toggle-fx-piano-roll`,
  `seq-show-fx-lower-panel`, arrangement mode via
  `eseq.piano-roll/set-arrangement-mode!`).

Engine / model:

- Track kinds: `ProjectTrackKind` (`project.rs`) / `InstrumentType`
  (`sequencer/data.rs`) = Empty, Sampler, Custom, Modulator, Rack.
- Arrangement: `ProjectSong` rows (`sequencer/state/song.rs`) on beats,
  per-track `LaneSource` = `Pattern | Take | Empty`.
- `hound` (WAV write) and `symphonia` (wav/aiff/flac/mp3 decode) are
  dependencies.
- RT hazard: the scheduler-snapshot seam has dropped large `Arc`s on the
  audio thread before (eseq-sj01). Region lists and source buffers must never
  be freed on the audio thread.
- **There is no audio input stream anywhere** in the crate; `recorder.rs`
  captures the master output only (relevant to phase 2).

## 3. Terminology

- **Source asset** — an immutable audio file owned by the project, identified
  by `AudioSourceId`. Never modified after it is written.
- **Region** — a placement of a span of one source on an audio track's lane.
  Ableton calls this an arrangement audio clip.
- **Clip editor** — the lower-panel waveform editor for one region (§8).
- **Sample core** — the playback code shared by sampler voices and the
  region player (§6).

## 4. Goals (phase 1)

- Add an audio track from the Add Track menu.
- Drag one or more samples from the browser sidebar onto an audio track;
  each becomes a region at the drop position, snapped to the grid.
- See regions as waveforms on the timeline.
- Move, trim either edge, split, duplicate, copy/paste (within and across
  audio tracks), delete, with undo — through the arrangement's existing
  selection and region gestures (Cmd+A, sweep, copy/paste).
- Double-click a region to open its waveform in the clip editor; drag start
  and end markers there; set clip gain.
- Open the track's FX panel exactly as for any other track; add fx.
- Save/load; bounce/export includes audio tracks.

## 5. Non-goals (phase 1)

- Recording (phase 2, §12–§13).
- Warping / time-stretch / tempo following (phase 3 — reuses the sampler's
  warp, §6.3).
- A device slot or modulation on audio tracks (phase 3, §6.3).
- Session/scene audio clips (phase 3).
- Disk streaming — sources load into memory.
- Render-to-new-source ops (reverse, normalize, consolidate).
- Clip looping inside a region.

## 6. Shared sample core

### 6.1 Extraction (first slice)

Move out of `instruments/sampler.rs` into a new directory module (e.g.
`instruments/sample_core/`), with the sampler calling it and its tests
unchanged:

| Piece | Today | After |
|---|---|---|
| decode + stereo fold | inside `load_wav_buffer` | `sample_core::decode_stereo(path) -> DecodedSample` |
| buffer registration | inside `load_wav_buffer` | `sample_core::register_buffer(lg, &DecodedSample) -> buffer id` |
| leading-silence trim | inside `load_wav_buffer` | stays in the sampler, applied before registration |
| rate step | `sampler_playback_step` | `sample_core::playback_step` |
| interpolated read | `read_interpolated` | `sample_core::read_interpolated` |
| peaks for drawing | widget reads buffer | `sample_core::peaks(...)` (cached, regenerable) for timeline rows |

The rule: **anything a sample is, lives in the core; anything a sampler voice
decides (trim, envelope, retrigger, loop modes), stays in the sampler.**
Behavior of the sampler must be bit-identical after the move; pin it with
the existing sampler tests plus one render comparison.

### 6.2 What the region player uses in phase 1

Only `playback_step` (native-rate sources, same ratio as the sampler) and
`read_interpolated`, plus fades and gain. No envelopes, no voice state.

### 6.3 Later: audio tracks expose sampler parameters

Long-term an audio track gets a **device**: a sampler-shaped parameter
surface (speed, SR reduction, scrub, warp, start/end offset) with the same
four external modulation inputs the sampler voice has (`ext1..4`), so an
audio track can be modulated like a sampler. To keep that open:

- Sampler DSP that is per-sample-of-playback (SR reduction, scrub smoothing,
  warp segment logic) should migrate into the core as **state-struct +
  function** pieces, not stay buried in `sampler_process`'s slot block, when
  that phase starts. Phase 1 does not move them.
- The region player's state layout must reserve room to grow (versioned or
  appended params, never reordered — same rule as `PARAM_SLOT_STRIDE`).
- Warp markers in the clip editor (§8.3) feed the same warp engine the
  sampler's Beats mode uses, not a second stretcher.

## 7. Getting audio onto a track

### 7.1 Drop targets

| Drop | On | Result |
|---|---|---|
| one or more `"sample"` | an existing audio track lane | one region per sample at the drop time, snapped to grid, laid end to end in drag order |
| `"sample"` | the new-track drop zone | **unchanged**: creates a sampler track, as today |
| `"instrument"`, `"sound"`, ... | an audio track lane | rejected (no drop highlight) |
| `"sample"` | a non-audio track lane | unchanged (current behavior) |

Audio lanes add `"sample"` to their `:drop-types`. *As built:* the drop
command (`audio-regions-drop`) takes `:paths` as well as `:path`, but the
browser tree has no multi-select yet, so dragging several samples at once
waits on that widget work (eseq-audio.19). A sample dropped on an audio
track's mixer strip lands at the arrangement cursor.

Snapping: *as built*, the drop beat floors to the beat grid (finer than the
bar snap scene drops use).

### 7.2 Import pipeline

On drop, per sample:

1. Copy the file's bytes into the audio store (§9.4) under its sha256 —
   WAV only in phase 1 (library samples are WAV).
2. Decode with the shared core (no silence trim), cache the stereo PCM, and
   register its waveform in the `eseqlisp` sample registry for drawing.
3. Register the `AudioSourceMeta` (deduplicated by file) and create the
   `AudioRegion` spanning the whole source at the drop beat.
4. All regions from one drop are **one undo step**.

*As built:* decode and copy are synchronous (library samples are small);
off-thread import with a "loading" placeholder is a follow-up.

Dragging files in from Finder uses the same pipeline (later slice).

## 8. UI

### 8.1 Timeline rows

- Audio tracks draw regions with a waveform: the `timeline` widget's
  `Waveform` content (`{:sample registry-key :offset s :span s}`) reads the
  registered sample's peak pyramid, so no peak data crosses into Lisp.
- *As built:* the lane (`audio-lane-action` in arrangement.lisp) handles
  title-bar select, double-click, title-bar move and edge trim through the
  shared ghost channel. A press in a region's BODY is a region surface, as on
  a clip: it parks the transport-start cursor where it lands and a drag
  sweeps a time selection (edges snap to the nearest grid line). The host's
  region verbs (`song_region_copy/paste/move/delete/duplicate`) carry audio:
  a slice of a region copies with its source offset advanced to the cut, so
  part of a region can be copied, pasted, deleted or duplicated. Cmd+E splits
  the region (or clip) under the cursor. Cross-track drag is a follow-up
  (eseq-audio.22).
- Audio content at or after the Play position counts as arrangement
  content: Play does not auto-latch the session scene over an audio-only
  arrangement.
- Edge trim on the timeline changes `source_offset` / the span (left edge
  trims into the source, it does not move content).

### 8.2 Clip editor (waveform)

Double-clicking a region (or its title bar) binds it and opens the **clip
editor** in the lower panel through
`seq-open-arrangement-piano-roll-bottom-for-track`. *As built:* the editor
(`content/ui/audio-clip.lisp`) renders inside the `*piano-roll*` buffer
whenever its track is an audio track, so every lower-panel toggle and layout
path reaches it unchanged — no new buffer.

v1 contents, Ableton-shaped:

- The whole source's waveform (the `waveform` widget bound to the source's
  buffer), with the region's span shown as start/end markers; the area
  outside the span is drawn inactive (`:inactive-waveform-color`).
- *As built (rev 2.2):* Ableton-shaped. A clip panel on the left — Start /
  End fields (seconds; drag or type, one undo step per drag), a Loop toggle,
  Gain, Pitch (semitones, warp off: speed follows), and disabled Warp /
  Reverse placeholders — beside the waveform. The waveform uses the widget's
  `:marker-style :bar`: a start/end bar in a strip under the seconds ruler
  (drag an end to trim, the middle to slide the window) and constant-width
  lines through the body, instead of the sampler's flags. Audio outside the
  window is drawn dimmed. Only the bar moves the markers: a drag across the
  waveform selects a range there (`:range-start`/`:range-end`) and the same
  stretch of the timeline as an arrangement region, so it can be copied,
  duplicated or deleted. Cursor and playhead are constant-width lines.
- On the timeline a looping region is drawn as one segment per repeat, as a
  pattern clip is (the waveform content reports its loop as a cycle).
- Setting the window keeps the region's timeline start and plays the chosen
  span (`audio-region-set-window`).
- Loop: `AudioRegion::loop_enabled` with a `[loop_start, loop_end)` window
  in source frames. Turning it on loops what the region plays now; while it
  loops the bar edits the loop, and extending the region on the timeline
  repeats the loop (the timeline waveform repeats with it).

### 8.3 Clip editor later

Warp markers and warp mode (feeding the sampler's warp engine, §6.3),
transpose, reverse, fades drawn on the waveform, loop brace.

### 8.4 FX panel

The gesture that opens a track's FX panel in the arrangement opens the same
`*fx*` buffer for an audio track: the fx chain, add-effect, bypass, reorder
— all unchanged. Where an instrument track shows its device, an audio track
shows nothing in v1. When §6.3 lands, that slot shows the audio track's
sampler-shaped device.

### 8.5 Track header / menus

- Add Track menu gains **Audio**.
- Header: name, mute/solo/volume as other tracks. (Arm/monitor/input arrive
  with recording.)

## 9. Data model

### 9.1 Types

*As built* (`sequencer/state/audio_regions.rs`):

```
AudioSourceMeta {           // ProjectArrangement::audio_sources[id]
    file: "<sha256>.wav",    // in the audio store (§9.4)
    name, sample_rate, channels, frames,
}

AudioRegion {               // ProjectArrangement::audio_lanes[track]
    id: ClipId,              // the clip allocator: selection treats regions like clips
    source: AudioSourceId,
    start_beat, end_beat: f64,   // the span, like a clip's
    source_offset: u64,      // source frame heard at start_beat
    gain_db, fade_in_seconds, fade_out_seconds: f32,
    muted: bool,
}

ProjectTrackKind::Audio     // unit variant; content lives in the arrangement
InstrumentType::Audio, TrackInstrumentSource::Audio
```

Regions living in `ProjectArrangement` gives them its validation, undo
patches (`edit_arrangement`) and track delete/move remaps for free. A lane
is sorted and non-overlapping: an incoming region truncates what it lands
on (`occlude_audio_span`), as Ableton's arrangement does, and the truncated
tail's offset advances so it keeps playing the same audio. Spans are beats;
the content plays at native speed, so a span longer than the audio left (a
tempo change can do that) plays silence past the end. An audio track keeps
(empty) scene patterns like every track; any clip scene stamping leaves on
its lane plays nothing (the audio thread skips Audio tracks) and the UI
draws only regions there.

- Positions on the timeline are **beats**; content is **frames**. Without
  warping, a tempo change moves a region's start but not its content speed
  (Ableton "warp off"). A loop dropped at the wrong tempo will not line up
  until warping exists — expected in phase 1.
- Recording's input selection will be a per-track field added in phase 2.

### 9.2 Playback

- One **region player** node per audio track (Rust `NodeVTable`), feeding
  the track fx chain like an instrument's voice sum.
- *As built* (`src/audio_track/`): each player node holds a slot whose
  `AtomicPtr<AudioTrackPlayback>` the control thread swaps
  (`App::sync_audio_track_playback`, every frame and after each edit, keyed
  on the region list, tempo and decoded sources). Snapshots hold
  `Arc<SourcePcm>`; a swapped-out snapshot is freed on the control thread
  only once the rendered-sample counter passes the value read at the swap,
  i.e. once any block that could have read it has finished — **never on the
  audio thread** (eseq-sj01).
- Per block the node reads `SongPositionShared::render_mapping` (song beat
  at the block's first sample, unwrapped, plus the pass start so pre-roll is
  silent) and per frame finds the region (hinted search), reads with
  `sample_core::read_stereo_at` at `playback_ratio(source_sr, host_sr)`, and
  applies gain and edge fades (at least 64 frames, so no edge clicks).
- A transport start ramps in, a stop ramps out from where it was, and a seek
  or loop wrap restarts the ramp instead of splicing.
- Bounce/export renders audio tracks through the same player (tested:
  sample-accurate onset on the region's beat).

### 9.3 Relation to rows, takes and scenes

Audio lanes are **not** compiled into `ProjectSong` rows and have no
`LaneSource`. Session playback ignores audio tracks in v1 (they play when
the arrangement plays). Audio takes (phase 2) are a separate `AudioTakeId`
space from note `TakeId`s.

### 9.4 Storage

- *As built:* projects here are single named JSON files in one folder, so
  a per-project folder would break for unsaved projects and Save As.
  Sources are copied into a content-addressed store in the user data root,
  `audio/<sha256>.wav` (`AppPaths::audio_dir`; `ESEQ_AUDIO_STORE_DIR`
  overrides it for tests and tools). The same file dropped twice is stored
  once, and the project still never depends on where the audio came from.
  Collecting the store's files into an exported/packaged project is a
  follow-up.
- Missing sources load as offline regions (drawn, silent), never an error
  that blocks project load — except a bounce, which refuses to render
  silence in their place.
- Unreferenced sources are left in place in phase 1; GC on save comes with
  render-to-new-source ops (§14).
- Serialization bumps the project version; older files load with no audio
  tracks.

## 10. Editing semantics

Region-only, undo via the existing history host-command pattern: move, trim
either edge, split, duplicate, copy/paste, delete, gain, fades, mute, nudge,
slip (move content under fixed edges = change `source_offset`). Drags commit
one undo step at gesture end (see the undo-drag invalidation pattern used by
arrangement clip drags).

Render-to-new-source (later, still undoable): reverse, normalize,
consolidate, bounce-in-place, time-stretch. The old source stays referenced
by history until GC.

## 11. Phasing

**Phase 1 — placed audio, Ableton parity (the first milestone)**
1. Extract the shared sample core from the sampler (§6.1), sampler
   behavior unchanged.
2. Audio track kind, model types, serialization, project audio folder,
   region player, bounce. Add Track › Audio.
3. Timeline rows draw regions with waveforms; drop samples (single and
   multi) onto an audio track to create regions (§7).
4. Region editing on the timeline: move/trim/split/duplicate/delete/gain,
   with undo.
5. Arrangement copy/paste, Cmd+A and region sweep include audio regions.
6. Clip editor: double-click opens the waveform editor; start/end markers,
   gain; FX↔editor toggle (§8.2).
7. FX panel opens for audio tracks; no device slot (§8.4).

**Phase 2 — recording** (rev 1's milestone, design in §12–§13)
1. Input stream + device/channel selection + input ring + `InputSource`
   (plain) + input node + input meter.
2. Rate/clock/permission checks on arm.
3. Arm / monitor / record tap / writer thread / latency-stamped region
   creation.
4. Loopback latency calibration + manual offset.
5. Import by dragging files from Finder.

**Phase 3**
- Audio track device: sampler-shaped params + ext1..4 modulation (§6.3).
- Warp markers / time-stretch via the sampler's warp engine; tempo following.
- Loop recording, punch in/out, take stacks + comping.
- Render-to-new-source ops; source GC on save.
- Cross-clock input via adaptive resampler.
- Disk streaming for long sources.
- Audio clips in session scenes.

## 12. Recording: input path (phase 2)

### 12.1 Shape

```
cpal input callback ──► input ring (SPSC, per device)
                              │
                      InputSource (§12.3)
                              │
          input node (Rust NodeVTable, 0 inlets, 1–2 outlets)
             │                               │
     armed track fx chain             record tap ──► writer ring ──► writer thread ──► WAV
     (monitoring, if on)              (raw, pre-fx)
```

- The **input node** runs inside the graph callback, so every block it reads
  is stamped with the *output* transport's sample position. That is what
  makes timeline alignment exact; recording in the cpal input callback would
  lose that relation.
- The **record tap is pre-fx**: the WAV holds the dry DI signal so amp-sim
  choices stay editable after recording.
- The **writer thread** drains the writer ring into `hound`. The audio thread
  never touches files, never locks, never allocates; a full writer ring
  counts a dropped block (same contract as `MasterRecorder`'s `try_lock`)
  and is surfaced to the user, never silently ignored.
- One input stream per open input device, shared by every armed track that
  selects a channel on it.
- On stop, the finished WAV becomes an `AudioSource` in the project audio
  folder and a region on the track — the same objects phase 1 creates by
  drop.

### 12.2 Device selection

- Input device + channel(s) are explicit and **saved with the project**
  (per audio track: device id, channel or channel pair). Never default to
  the system default input: on the dev machine that is BlackHole, a virtual
  loopback device, which would record silence.
- Device list comes from cpal; channel count from the device's input config.

### 12.3 The `InputSource` boundary

```
read(block: &mut [f32], frames) -> InputRead { frames_read, underrun }
latency_frames() -> u32      // fixed lag this source adds (feeds §13)
```

v1 implementation: **plain ring with a fixed safety margin.** The node
starts reading `margin` frames behind the writer so input/output callback
jitter never empties it. The margin is a latency term.

### 12.4 Clocks and rates

cpal has no duplex stream, so even one device yields two callbacks. With a
shared clock that is fine; across clocks the ring drifts (tens of ppm → a
click every few minutes) and a rate mismatch empties it almost immediately.

On arm, check:

1. **Rate.** If the input device's nominal rate ≠ the graph rate, set the
   input device's nominal rate to match (CoreAudio
   `kAudioDevicePropertyNominalSampleRate`). If that fails, refuse to arm
   and say why. (Dev machine: MacBook Pro Microphone 44.1 kHz vs Speakers
   48 kHz.)
2. **Clock.** Same device → OK. Different devices → compare
   `kAudioDevicePropertyClockDomain`; equal and nonzero → OK. Otherwise
   refuse to arm with "input and output run on different clocks — use one
   interface, or create an Aggregate Device with drift correction in Audio
   MIDI Setup."

Cross-clock follow-up (phase 3): an adaptive resampler inside `InputSource`
watches ring fill and nudges its ratio to hold a target fill. Nothing
outside `InputSource` changes; its resampler delay joins `latency_frames()`.

Linux: v1 ships the same-device rule with the clock check reduced to "same
device".

### 12.5 Monitoring

- Per audio track: **arm** and **monitor** (`off | auto | on`; auto = on
  while armed and not playing back a region at that point).
- Monitoring routes the input node into the track fx chain → mixer.
- When input and output are both built-in devices, monitor defaults to
  **off** (mic + speakers feed back).
- Latent fx on an armed track delay monitoring by their PDC; v1 shows the
  monitoring latency in the track header (§14).

### 12.6 macOS permission

Without `NSMicrophoneUsageDescription` and TCC approval, the input stream
opens and delivers **silence**. The app bundle needs the usage string;
unbundled dev runs get the prompt on behalf of the terminal. Detect an
all-zero input after arm and point at System Settings › Privacy ›
Microphone rather than recording silence silently.

### 12.7 Recording semantics

- Arm one or more audio tracks; transport Record starts all armed inputs.
- On start, the input node records the transport sample position of the
  first recorded block; on stop the writer finalizes the WAV and the control
  thread creates `AudioSource` + `AudioRegion` with `start_beat` from the
  latency-corrected position (§13).
- Recording over an existing region places the new region on top
  (non-destructive). Phase 3 turns this into a take stack.
- Undo of a recording removes the region; the source file is kept until GC.

## 13. Recording: latency (phase 2)

| Term | What | Owner |
|---|---|---|
| Graph PDC | latent fx padding inside the graph | existing `app/graph/latency.rs` |
| Output device | graph → speaker | today: one block (estimate) |
| Input device | mic/DI → input callback | new |
| Input ring margin | `InputSource::latency_frames()` | new |

```
recorded_pos = stamped_pos − (output_device + input_device + margin)
```

Graph PDC is **not** subtracted: recorded audio enters the graph at the
track and is compensated on playback like every other track.

Calibration: a loopback ping. Play an impulse out (the impulse node from the
`latency.rs` tests), record the input, cross-correlate, store the measured
round-trip per (input device, output device, rate, block size). It replaces
the output + input device terms; margin is still added. Uncalibrated
fallback: output block + input block + margin, plus a manual offset (ms).

The note/take record path keeps `total_record_latency_seconds`
(`sequencer/state/sequencer_state/transport.rs`) unchanged; audio recording
gets its own total, because MIDI input has no input-device term.

## 14. Open questions

1. ~~Clip editor buffer~~ — settled: inside `*piano-roll*` (§8.2).
2. **Multi-sample drop layout**: end to end on one track (built in the
   command), or one sample per track below when dropped on the new-track
   zone with a modifier?
3. **Source GC**: delete unreferenced sources on save, or move to a trash
   folder for one session?
4. **Rack membership**: can an audio track be a rack member? (Leaning no.)
5. **Low-latency monitoring** (phase 2): bypass PDC for armed+monitored
   tracks, as Ableton's "reduced latency when monitoring" does?
6. **Print fx while recording** (phase 2): optional post-fx tap?
7. **Linux** (phase 2): CPAL/PipeWire input latency and clock detection.

## 15. Beads

Epic `eseq-audio`. Phase 1 slices are P2; phase 2 (recording) and phase 3 are P3.

| Bead | Slice | Phase | Depends on |
|---|---|---|---|
| eseq-audio.14 | extract shared sample core from sampler | 1 | — |
| eseq-audio.3 | track kind, model, serialization, audio folder, region player, waveform rows | 1 | .14 |
| eseq-audio.6 | drop samples (single + multi) onto audio track → regions | 1 | .3 |
| eseq-audio.7 | region editing on the timeline + undo | 1 | .3 |
| eseq-audio.15 | arrangement copy/paste/Cmd+A/sweep include audio regions | 1 | .7 |
| eseq-audio.16 | clip editor: double-click waveform editor, start/end, gain | 1 | .3 |
| eseq-audio.17 | FX panel for audio tracks, no device slot | 1 | .3 |
| eseq-audio.8 | bounce/export renders audio tracks | 1 | .3 |
| eseq-audio.1 | input stream, device/channel, `InputSource`, input node, meter | 2 | .3 |
| eseq-audio.2 | arm-time rate/clock/permission checks | 2 | .1 |
| eseq-audio.4 | arm/monitor, record tap, writer, stamped region creation | 2 | .1 |
| eseq-audio.5 | loopback calibration + manual offset | 2 | .4 |
| eseq-audio.18 | audio track device: sampler params + ext1..4 modulation | 3 | .14, .17 |
| eseq-audio.9 | cross-clock input (adaptive resampler) | 3 | .1 |
| eseq-audio.10 | loop/punch recording, take stacks, comping | 3 | .4 |
| eseq-audio.11 | render-to-new-source ops + source GC | 3 | .3 |
| eseq-audio.12 | disk streaming | 3 | .3 |
| eseq-audio.13 | warp markers (sampler warp engine) + session clips | 3 | .16, .18 |
