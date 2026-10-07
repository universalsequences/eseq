# Microtonal scales + scale editor — spec rev 1

Epic: `eseq-th7i` (filed 2026-10-02), children `.1`–`.7` map to §7 phases 1–7.

## 1. Goal

The track settings `scale` dropdown (Fit To Scale, `fts_scale`) only knows 13
12-tone scales stored as whole semitones. We want:

1. Exotic and microtonal scales in the dropdown (just intonation, historical
   temperaments, maqam, gamelan, N-EDO, non-octave scales).
2. A gear button beside the dropdown that swaps the `*track*` buffer to a
   **scale editor**: see the scale, drag individual notes sharp/flat in cents,
   switch notes off, and apply quick whole-scale edits (just-ify, randomize,
   stretch), plus a single **morph** control from "standard tuning" to "the
   tuning you built".
3. Scala `.scl` import, so any of the published tunings can be loaded.

Out of scope: MIDI output (eseq has no MIDI out today — `midir` is only used
for input), per-note MPE, and retuning the piano roll grid.

## 2. What exists

- `crates/sequencer/src/scale.rs`: `ScaleDef { name, degrees: &[u8] }`,
  `SCALES`, `quantize_transpose(transpose, scale_idx)` — nearest-degree snap,
  root fixed at C (transpose 0).
- Callers: `scheduler/process.rs::apply_fit_to_scale_to_trigger` (sequenced
  notes + chord voices, reads the published snapshot) and
  `audio/params.rs::resolve_live_keyboard_transpose` (live keys, audio thread).
- Pitch is `f32` semitones end to end (`ResolvedStep.transpose`,
  `ScheduledChordData.notes`, `VoiceSlot.note`, `440·2^((t−9)/12)`), so a
  fractional result reaches every engine without further plumbing.
- `fts_scale` is a persisted **index** (`ProjectTrackParams.fts_scale`). New
  scales are therefore **append-only**; reordering would retune old projects.
- Track params live as atomics on `TrackParams`, are copied into
  `TrackParamsSnapshot` (undo witness, scene/sound capture, scheduler
  snapshot), and are edited through `AppCommand::SetTrack*` +
  `slice3-history-action`.

## 3. Data model

### 3.1 Scale table (`scale.rs`)

```rust
pub struct ScaleDef {
    pub name: &'static str,
    pub cents: &'static [f32],   // ascending, cents[0] == 0
    pub period: f32,             // repeat interval in cents (1200 = octave)
    pub mode: TuningMode,        // default input mapping when picked
}
```

Indices 0..=12 keep today's scales (cents = semitones × 100, period 1200,
mode Snap), so every existing project quantizes bit for bit the same. New
entries are appended (§6).

### 3.2 Per-track tuning (`TrackTuning`)

Lives beside `fts_scale` on each track:

| field      | type                   | meaning |
|------------|------------------------|---------|
| `root`     | `u8` 0..11             | pitch class degree 0 sits on (C = 0). |
| `morph`    | `f32` 0..1, default 1  | 0 = every tuned degree rounded to the nearest 12-TET semitone, 1 = exact. Linear in cents between. |
| `mode`     | `Snap \| Map`          | input mapping (§4). |
| `offsets`  | `[f32; 64]` cents      | per-degree detune added to the base scale. |
| `disabled` | `u64` bitmask          | degrees removed from the scale. |
| `custom`   | `Option<Arc<CustomScale>>` | an imported `.scl` that replaces the base table. |

`CustomScale { name, cents: Vec<f32>, period }`. When `custom` is `Some` it is
the base table and the dropdown shows its name; picking any built-in scale
clears it.

**Picking a scale resets** `offsets`, `disabled` and `custom`, and sets `mode`
to the scale's default. `root` and `morph` are kept. Undo restores all of it
because `TrackTuning` is part of `TrackParamsSnapshot`.

`MAX_SCALE_DEGREES = 64` (covers 53-EDO). Longer `.scl` files are rejected
with a message, not truncated.

### 3.3 Threading

`TrackParams.tuning` is a `Mutex<TrackTuning>` (UI-thread writes, like
`midi_fx_chain`). The audio thread never locks it: both quantize sites read
`snapshot.tracks[i].params.tuning`, which the normal snapshot publish copies.

### 3.4 Persistence

`ProjectTrackParams.tuning: Option<ProjectTrackTuning>` — `None` (omitted)
when default, so untouched projects serialize unchanged. Offsets and disabled
degrees are stored sparsely (`[[degree, cents], …]`, `[degree, …]`); the
custom scale inline (`{name, cents, period}`). Sounds (`sound_entities.rs`)
carry it alongside `fts_scale`.

## 4. Quantization

All math in cents relative to `root`. `pitch(k)` for degree `k` of the
enabled degree list is

```
tuned  = base[k] + offsets[k]
et     = round(tuned / 100) * 100
pitch  = et + (tuned - et) * morph
```

**Snap** (default; today's behavior): `x = (t − root)·100`, `oct =
floor(x / period)`, pick the enabled degree whose `pitch` is nearest to
`x − oct·period`, also testing degree 0 of the next period and the last
degree of the previous one. Result `root + (oct·period + pitch)/100`.
With root 0, morph 1, no offsets and a legacy scale this is exactly
`quantize_transpose`.

**Map**: each semitone of input is the next enabled degree.
`k = floor(t − root)`, `frac` = remainder, `m` = enabled count,
`pitch_at(k) = div_euclid(k, m)·period + pitch(rem_euclid(k, m))`, result
`root + lerp(pitch_at(k), pitch_at(k+1), frac)/100`. This is how scales with
more or fewer than 12 notes per period (19-EDO, Bohlen-Pierce, Carlos α) are
fully reachable from steps and a 12-key keyboard.

If every degree is disabled the input passes through unchanged.

## 5. UI

### 5.1 Track settings

`content/ui/effects/track-panels.lisp`: a small gear button right of the
`scale` dropdown opens the scale editor (`eseq.effects.scale-editor`'s
`scale-view.open` view singleton; it edits the track's `tuning` kind). While
open, the `*track*` buffer renders `scale-editor-panel` instead of
`track-parameters-panel`. The dropdown shows the custom scale name when one
is loaded, and a `*` suffix when the tuning has edits.

### 5.2 Scale editor panel

```
[‹ settings] scale [Just Major ▾]  root [C ▾]  [Snap|Map]  morph [100%]  [.scl…] [reset]
┌ staircase: one step per degree over the period, 12-TET grid behind ────┐
├ detune bars: one column per degree, centre = base pitch, drag ±range ──┤
│ ▲ +14¢     ▼ −31¢ …   ●  ●  ○  ●   (click dot = enable/disable)       │
└────────────────────────────────────────────────────────────────────────┘
tools: [just] [rand ±15¢] [stretch +5¢]      deg 3 · 386.3¢ · −13.7¢ vs ET
```

New Rust widget `scale-editor` (`crates/eseqlisp/src/widget_render/
scale_editor.rs`), plain rects/circles/text:

- props: `:base` (cents per degree), `:offsets`, `:enabled` (bools),
  `:period`, `:morph`, `:range` (± cents of the bar area, default 100),
  `:selected`, `:labels`, `:on-change`.
- Top ~35%: staircase of effective pitch per degree over one period, with
  faint 12-TET lines — "see the scale".
- Bottom: detune bars. Press/drag in a column dispatches `(:set k cents)`,
  release `(:finish k cents)`, double-click or alt-press `(:clear k 0)`,
  press on the bottom dot row `(:toggle k 0)`. A faint tick marks where the
  nearest 12-TET semitone sits, so "how far from ET" is visible.

### 5.3 Tools

All operate on enabled degrees and are one undo step each:

- **just**: move each degree to the nearest ratio in a fixed 7-limit table
  (1/1 16/15 10/9 9/8 8/7 7/6 6/5 5/4 9/7 4/3 7/5 10/7 3/2 14/9 8/5 5/3 12/7
  7/4 16/9 9/5 15/8 2/1) if it is within 50¢, else leave it.
- **rand ±N¢**: add uniform noise in ±N to each offset.
- **stretch +N¢**: `offset[k] += N · base[k] / period` (piano-style stretch
  within the period; degree 0 never moves).
- **reset**: clear offsets and disabled degrees.

### 5.4 Natives and history

- `(seq-tuning op value degree)` enqueues host command `track-tuning-action`
  with `{op, track, value, degree}`. The host reads the current `TrackTuning`,
  applies the op and issues `AppCommand::SetTrackTuning { track, tuning,
  edit }`.
- History: `offset` coalesces per `track:{t}:tuning-offset:{k}` and `morph`
  per `track:{t}:tuning-morph` (like swing); every other op records.
- Reactive fields (current track): `SEQ.tp-tuning-base`, `-offsets`,
  `-enabled`, `-period`, `-root`, `-morph`, `-mode`, `-edited`,
  `-scale-name`, `SEQ.tuning-root-options`.

## 6. Appended scales (index 13+)

| name | cents | period | mode |
|---|---|---|---|
| Harmonic Minor | 0 200 300 500 700 800 1100 | 1200 | Snap |
| Melodic Minor | 0 200 300 500 700 900 1100 | 1200 | Snap |
| Hungarian Minor | 0 200 300 600 700 800 1100 | 1200 | Snap |
| Phrygian Dom. | 0 100 400 500 700 800 1000 | 1200 | Snap |
| Hirajoshi | 0 200 300 700 800 | 1200 | Snap |
| In (Miyako) | 0 100 500 700 800 | 1200 | Snap |
| Just Major | 1 9/8 5/4 4/3 3/2 5/3 15/8 | 1200 | Snap |
| Just Minor | 1 9/8 6/5 4/3 3/2 8/5 9/5 | 1200 | Snap |
| Just Chromatic | 5-limit 12-note | 1200 | Snap |
| Pythagorean | 3-limit major | 1200 | Snap |
| Meantone ¼ | quarter-comma 12 | 1200 | Snap |
| Werckmeister III | 12 | 1200 | Snap |
| Harmonic 8-15 | 8/8 … 15/8 | 1200 | Snap |
| Maqam Rast | 0 200 350 500 700 900 1050 | 1200 | Snap |
| Maqam Bayati | 0 150 300 500 700 800 1000 | 1200 | Snap |
| Maqam Saba | 0 150 300 400 700 800 1000 | 1200 | Snap |
| Pelog | 0 120 270 540 670 785 950 | 1200 | Snap |
| Slendro | 0 240 480 720 960 | 1200 | Map |
| 7-EDO | 7 equal | 1200 | Map |
| 19-EDO | 19 equal | 1200 | Map |
| 22-EDO | 22 equal | 1200 | Map |
| 24-EDO | 24 equal | 1200 | Map |
| 31-EDO | 31 equal | 1200 | Map |
| Bohlen-Pierce | 13 equal | 1901.955 | Map |
| Carlos Alpha | 0 | 78.0 | Map |
| Carlos Beta | 0 | 63.8 | Map |
| Carlos Gamma | 0 | 35.1 | Map |

## 7. Phases (beads)

1. **Core model** — `ScaleDef` in cents, appended scales, `TrackTuning`,
   snapshot/undo/persist/sounds, Snap/Map/root/morph quantizer, both quantize
   sites. Tests: legacy-scale bit-exact regression, Just Major E → 386.31,
   morph 0 = 12-TET, Map 19-EDO, root shift, disabled degrees, round-trip.
2. **Editor commands** — `SetTrackTuning`, `track-tuning-action` ops incl.
   tools, coalescing, reactive fields, scale-name/`*` dropdown label.
3. **scale-editor widget** — renderer + mouse events + tests.
4. **Editor panel** — gear button, panel swap, tools row, readout; verified
   with `metal_seq capture`.
5. **Scala import** — `.scl` parser (cents + ratio lines, comments, period
   = last line), `.scl…` button with NSOpenPanel, error toast.
6. **Morph p-lock** (follow-up) — per-step morph lock so a phrase can sweep
   from in-tune to detuned.
7. **Docs** — manual section for scale editor.
