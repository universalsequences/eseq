# Sample Warp v0.2 — Warp Modes Spec

> **Implementation status (2026-07):** Re-Pitch, the Beats rewrite, and
> Preserve=Transients are implemented. Key decisions that diverged from the
> plan below, in the direction of less plumbing:
>
> - **No `WarpGridShared`.** The uniform grid is pure arithmetic, so the audio
>   thread computes boundaries on the fly from `sample_bpm` + preserve
>   (`src/instruments/warp_grid.rs`, pure functions + tests). No
>   publish/rebuild channel; bpm and preserve edits take effect live.
>   Preserve=Transients snaps each grid boundary to the nearest onset (±25 ms) via binary search over the
>   already-published `OnsetTableShared` — snapping only, no onset insertion.
> - **The grid anchors at the region start point** (trim start to the downbeat
>   to align the grid), not an analysis-provided downbeat. Downbeat anchoring
>   is still a follow-up.
> - **Warp no longer requires analysis.** Beats runs on the pure grid when no
>   onset table exists, so racks support all warp modes and warp engages the
>   moment a bpm is known/typed.
> - Params landed as `preserve` / `fill` (off, loop, ping-pong) / `decay`
>   (0..1), appended at the descriptor tail (after `smooth`) to keep plock
>   indices of older params stable. Tones/Texture currently fall back to
>   Re-Pitch until the granular engine lands.
> - Mid-note tempo change re-derives position closed-form from
>   `gate_counter * ratio` (no slice-walking), then normal segment advancement
>   takes over.

## Problem

The v0.1 warp engine (see `sample-warp-v0.1-spec.md`) is transient-locked
slice playback driven entirely by aubio's onset table. In practice it is only
usable on clean breaks near the original tempo, for two structural reasons:

1. **Onsets are the only timing source.** A missed onset means a whole musical
   subdivision plays at the wrong time; a spurious onset means a slice fires
   mid-note. Aubio's onset detector is lossy on real material (ghost notes,
   reverb tails, layered breaks), so the warp timing inherits that noise. There
   is no underlying beat grid to fall back on.
2. **There is no time stretching at all.** A slice either rings into silence
   (slow-down) or gets chopped (speed-up). This is unusable for pads, vocals,
   basslines, or any sustained material, and even on drums it produces gaps at
   larger down-stretches.

Ableton solves this with a *family* of modes, each tuned to a material type:

| Ableton mode | Material | Core technique |
|---|---|---|
| Beats | drums, breaks | grid-segmented native-rate playback + segment loop/envelope fill |
| Tones | monophonic pitched (bass, vocal) | granular SOLA, large correlated grains |
| Texture | polyphonic/ambient (pads, field rec.) | granular, free grain size + randomization |
| Re-Pitch | anything, DJ-style | plain resampling; pitch follows tempo |
| Complex / Complex Pro | full mixes | phase vocoder (+ formant handling) |

This spec defines the eseq equivalents. The target is Ableton-at-70%: Beats
that actually locks to a grid, Re-Pitch for free, a single granular engine
that covers both Tones and Texture, and a pragmatic path to Complex. All
realtime processing stays in `sampler_process` on the audio thread with no
allocations, using the existing atomic-slot state convention.

## Goals

- Replace onset-driven timing with a **beat grid** derived from
  `sample_bpm` + `downbeat_frame`, with detected transients as an optional
  refinement rather than the backbone.
- Four user-facing warp modes: **Beats**, **Tones**, **Texture**, **Re-Pitch**
  (Complex is phased in later, see *Further Steps*).
- Beats mode gets Ableton's `Preserve` control (1 Bar … 1/32, Transients),
  transient loop mode (Off / Loop / Ping-Pong) and a per-segment decay
  envelope — eliminating the silence-gap artifact at slow-downs.
- Tones/Texture share one granular SOLA engine; Tones adds splice-point
  correlation search, Texture adds grain-size + flux (randomization) controls.
- Re-Pitch folds the warp ratio into the resample rate; ~30 lines, ships first.
- Optional **transport anchoring**: a note triggered mid-bar can start the
  sample at the musically-corresponding position (the WarpEffect.js
  `stepNumberTriggered` behavior), so retriggering a warped break stays in
  phase with the pattern.
- Mode/params are per-sampler, plumbed through the existing aux-event +
  atomic-slot pathway, persisted in the project like every other sampler param.

## Non-Goals

- Full warp-marker editing (multiple user-placed markers with piecewise
  tempo). The grid is uniform: one BPM, one downbeat anchor. (Marker editing
  is sketched in *Further Steps* — the segment math below is written so a
  marker table can replace the uniform grid without touching the playback
  code.)
- Formant preservation / Complex Pro envelope controls.
- Warping in reverse playback (same restriction as v0.1; reverse disables
  warp). Ping-pong *within* a Beats segment is in scope; global reverse is not.
- Tempo ramp automation smoothness guarantees beyond the existing 50 ms ratio
  slew.
- Live-input warping. Buffers only.

## Shared Foundation

### The beat grid replaces the onset table as the timing backbone

Analysis already produces `bpm`, `bpm_confidence`, `onsets_frames`,
`downbeat_frame`. v0.2 adds a derived, precomputed **`WarpGrid`** that ships
to the audio thread the same way `OnsetTableShared` does today (Arc + packed
pointer pair). It replaces `OnsetTableShared` in the voice hot path;
`OnsetTableShared` remains for waveform-marker rendering.

```rust
pub struct WarpGridShared {
    /// Segment boundaries in SOURCE frames, strictly increasing,
    /// first entry <= region start when clamped, last entry >= region end.
    /// Uniform grid for Preserve=division modes; transient-snapped for
    /// Preserve=Transients. Regenerated (new Arc) whenever bpm override,
    /// downbeat nudge, preserve setting, or re-analysis changes.
    pub boundaries_frames: Vec<u32>,
    /// Frames per beat in source frames (60/bpm * source_sr). f64 to avoid
    /// drift over long samples.
    pub frames_per_beat: f64,
    /// Source frame of beat zero (downbeat anchor).
    pub anchor_frame: f64,
    pub sample_len_frames: u32,
}
```

Construction (control thread, in a new `warp_grid.rs`):

- **Preserve = 1 Bar / 1/2 / 1/4 / 1/8 / 1/16 / 1/32**: boundaries at
  `anchor_frame + k * frames_per_beat * beats_per_division` for all k such
  that the boundary lands in `[0, sample_len)`. Extend backwards from the
  anchor too (k negative) so samples that start before their first downbeat
  still get boundaries. `beats_per_division`: 1 Bar = 4.0 (assume 4/4; time
  signature is a follow-up), 1/2 = 2.0, 1/4 = 1.0, 1/8 = 0.5, 1/16 = 0.25,
  1/32 = 0.125.
- **Preserve = Transients**: start from the uniform 1/16 grid, then for each
  grid point, if a detected onset lies within ±25 ms, snap the boundary to
  the onset; otherwise keep the grid point. Then insert any remaining strong
  onsets that are > 30 ms away from every existing boundary. Finally dedupe
  boundaries closer than 20 ms. This inverts v0.1's failure mode: a missed
  onset now degrades to "grid-perfect but not transient-snapped" instead of
  "wrong timing", and a spurious onset adds one extra (harmless, still
  crossfaded) segment.

The grid is what makes the user's WarpEffect.js approach work: position in
the sample is a pure function of musical time elapsed, `resolution` is the
`Preserve` division, and detection noise can no longer shift timing.

### The master position equation

All modes share one definition of the ideal source read position. Per voice:

```text
musical_frames_elapsed = gate_counter            # host frames since trigger
                         (+ anchor_offset_frames if transport-anchored)
ideal_source_frame = warp_start_frame
                   + host_frames_to_source_frames(musical_frames_elapsed)
                     * warp_ratio
warp_ratio = project_bpm / sample_bpm            # v0.1 convention, unchanged
```

**Ratio orientation (the classic bug — write the test first).**
`warp_ratio = project_bpm / sample_bpm` is *source frames consumed per
musical frame*. Sanity check: a 174 BPM break in a 120 BPM project — one
project beat is 0.5 s of output covering one sample beat = 0.345 s of source,
so the read head advances at 0.345/0.5 = 0.69 = 120/174 ⇒ ratio < 1 means
slow-down, source consumed slower. It is very easy to convince yourself of
the inverse; the implementation plan mandates a unit test pinning this
number.

`ideal_source_frame` is *the* contract: at any host time, every mode places
its output so that the content at `ideal_source_frame` is what the listener
hears. Beats approximates it with native-rate segments, Tones/Texture with
grains, Re-Pitch exactly (by construction), Complex exactly (by vocoder).

### Transport anchoring (optional, per-sampler toggle)

`warp_anchor: note | transport`. `note` (default) behaves like today: the
sample starts from `start_sample` at note-on. `transport` computes, at trigger
time on the scheduler thread:

```text
beats_into_pattern = (pattern_step_at_trigger / steps_per_beat)
anchor_offset_frames = beats_into_pattern * host_frames_per_beat
```

and passes it as a new aux value; the audio thread adds it to
`musical_frames_elapsed`, wrapping by the warped sample length
(`sample_beats * host_frames_per_beat`). Result: hitting a note on beat 3
plays the break's beat-3 content — the WarpEffect.js behavior that made
sliced-break retriggers feel right. The scheduler already knows the step
number at trigger; this is one extra aux slot
(`SAMPLER_EVENT_AUX_WARP_ANCHOR_OFFSET`).

### State & event plumbing

New/changed atomic slots (indices assigned at implementation time; bump
`SAMPLER_STATE_SIZE`):

| Slot name | Meaning |
|---|---|
| `STATE_WARP_MODE` | 0 = Beats, 1 = Tones, 2 = Texture, 3 = Re-Pitch, 4 = Complex (reserved) |
| `STATE_WARP_PRESERVE` | 0..5 = 1Bar…1/32, 6 = Transients (Beats only) |
| `STATE_WARP_SEG_LOOP_MODE` | 0 = Off, 1 = Loop, 2 = Ping-Pong (Beats only) |
| `STATE_WARP_SEG_ENVELOPE` | 0..1, per-segment decay amount (Beats only) |
| `STATE_WARP_GRAIN_SIZE_MS` | Tones/Texture grain size, 8..250 ms |
| `STATE_WARP_FLUX` | 0..1 Texture randomization |
| `STATE_WARP_ANCHOR_OFFSET` | host frames, from scheduler (transport anchor) |
| `STATE_WARP_GRID_PTR_LO/HI` | packed `*const WarpGridShared` |
| + per-mode runtime state | see mode sections (segment index, grain phases…) |

`STATE_WARP_ONSET_TABLE_PTR_*` slots stay (waveform UI + Transients grid
construction happen control-side, so the audio thread actually no longer needs
the onset table at all — remove its audio-thread read once Beats is migrated).

Runtime state must fit the "reset on retrigger / on ratio jump" pattern the
v0.1 code already has (`reset_forward_warp_state`); each mode defines its own
reset.

Aux events gain `SAMPLER_EVENT_AUX_WARP_PRESERVE`, `..._SEG_LOOP_MODE`,
`..._SEG_ENVELOPE`, `..._GRAIN_SIZE`, `..._FLUX`, `..._ANCHOR_OFFSET`,
`..._GRID_PTR_LO/HI`, mirroring the existing warp aux block in
`sampler_schedule_event`.

`STATE_WARP_SAMPLE_BPM` modulation lanes (`MOD_WARP_BPM_LANES`) keep working
in all modes — they modulate the ratio, and each mode consumes the ratio
through the shared slewed `warp_ratio` (existing 50 ms slew stays).

---

## Mode 3: Re-Pitch (ship first)

Playback rate is multiplied by the warp ratio; no segmentation, no stretching.
Pitch shifts with tempo exactly like vinyl. In `sampler_process`:

```rust
let playback_step = sampler_playback_step(source_sample_rate, sample_rate, step_rate)
    * if warp_mode == MODE_REPITCH && warp_enabled { warp_ratio } else { 1.0 };
```

- All the v0.1 warp branches (slice advance, warp_silent, warp xfade) are
  bypassed in this mode; loop modes, reverse, scrub all work unchanged since
  it's just a rate change.
- `transpose` stacks on top multiplicatively (tune the repitched sample).
- Zero artifacts at any ratio; the entire diff is ~30 lines plus mode
  plumbing. This alone covers the classic jungle workflow (140→160 breaks
  with the pitch ride) and gives users an escape hatch whenever the fancier
  modes misbehave.

## Mode 0: Beats (rewrite of v0.1)

### Segment timing

Identical skeleton to v0.1 but boundaries come from `WarpGridShared`
(grid/transient-snapped), not raw onsets:

```text
on trigger / reset:
    seg = index of first boundary > warp_start_frame (clamped into region)
    seg_source_start = warp_start_frame
    seg_host_start = 0

per host frame:
    next_boundary_src = boundaries[seg]  (or end_sample if exhausted)
    next_boundary_host = seg_host_start
        + source_frames_to_host_frames(next_boundary_src - seg_source_start)
          / warp_ratio
    if musical_frames_elapsed >= next_boundary_host:
        start 5 ms equal-power xfade from current tap to next_boundary_src
        seg_source_start = next_boundary_src
        seg_host_start = next_boundary_host      # ← boundary time, NOT "now"
        seg += 1
```

One deliberate fix vs v0.1: `seg_host_start` is set to the *scheduled*
boundary time, not the current frame (`gate_counter`), so quantization error
does not accumulate across segments. v0.1's `slice_project_frame_start =
gate_counter` drifts by up to one frame per slice.

Within a segment the read head advances at **native rate** (`playback_step`,
including transpose) from `seg_source_start` — transients keep their attack
at any tempo, exactly as in v0.1.

### Filling time when slowing down (ratio < 1)

This replaces v0.1's emit-silence rule (`warp_silent`) with Ableton's
transient-loop behavior. When the read head reaches the next boundary before
its scheduled host time, instead of going silent:

- **Loop Off** (v0.1 behavior, kept as an option): hold silence after the
  natural content runs out — but apply the segment envelope (below) so it
  fades rather than truncates.
- **Loop** (default): define the segment's *loop tail* as its final portion:
  `tail_start = seg_source_start + 0.5 * (next_boundary_src - seg_source_start)`
  (midpoint; avoids re-firing the transient). When the read head hits
  `next_boundary_src`, jump back to `tail_start` with a 5 ms equal-power
  xfade (reuse the existing xfade machinery — it's the same dual-tap
  mechanism) and keep playing forward; repeat until the next segment fires.
- **Ping-Pong**: same tail region, but reverse direction at each tail edge
  instead of jumping. No xfade needed at the turnaround if direction flips on
  the exact sample (slope is continuous enough); keep a 2 ms fade as a
  safety.

New runtime state: `STATE_WARP_SEG_TAIL_DIR` (±1) and the tail bounds are
derivable, so only one extra slot.

### Per-segment envelope

`STATE_WARP_SEG_ENVELOPE` (0..1, default 0 = off). When > 0, each segment's
amplitude is multiplied by an exponential decay restarting at every segment
boundary:

```text
seg_env = exp(-t_in_segment_host / (tau))
tau = lerp(seg_host_len * 4.0, seg_host_len * 0.15, envelope)
```

At `envelope = 0` decay is negligible; at 1 each segment gates down hard —
the classic "choppy" Beats sound, and it masks loop-tail repetition at
extreme slow-downs. Multiply into `env_amp` alongside the main envelope.

### Speeding up (ratio > 1)

Unchanged from v0.1: the next segment fires before this one finishes; the old
tap fades over the 5 ms xfade. At high ratios (>2) segments get very short;
the grid guarantees they're still musically placed (v0.1's onset gaps made
this fall apart).

### Preserve = Transients

Same playback code; the grid was transient-snapped at construction. No
audio-thread difference — this is the payoff of moving onset handling to the
control thread.

## Modes 1–2: Tones & Texture (granular SOLA engine)

One engine, two parameter presets. This is time-domain synchronized
overlap-add: two read taps ("grains") crossfading, output at native pitch,
grain start positions tracking `ideal_source_frame`.

### Core loop

Per-voice runtime state: `grain_a_pos`, `grain_b_pos` (source frames, advance
at `playback_step` per host frame), `grain_phase` (0..grain_len host frames),
`grain_len_host`, `rng_state` (xorshift32 in an f32 slot's bits).

```text
per host frame:
    ph = grain_phase / grain_len_host              # 0..1
    w_b = sin(ph * PI/2)^2                         # equal-power-ish (raised cos)
    w_a = 1 - w_b
    out = read(grain_a_pos) * w_a + read(grain_b_pos) * w_b
    grain_a_pos += playback_step
    grain_b_pos += playback_step
    grain_phase += 1
    if grain_phase >= grain_len_host:
        grain_a_pos = grain_b_pos                  # b becomes the old grain
        target = ideal_source_frame(now) + jitter  # re-sync to master clock
        grain_b_pos = splice_search(target)        # Tones only; Texture: target
        grain_phase = 0
```

Grain length: `grain_len_host = grain_size_ms * host_sr / 1000`, halved
overlap (each frame is always inside exactly one crossfade — "50% overlap,
two taps" — the standard cheap SOLA). Since a new grain spawns at the ideal
position every `grain_len_host` frames, position error never exceeds one
grain, and pitch is always native.

- **ratio < 1 (slow-down)**: consecutive grains overlap in source →
  content repeats at grain granularity (the familiar granular smear).
- **ratio > 1 (speed-up)**: grains skip source content.
- **ratio = 1**: `target` always equals the natural continuation, so
  `grain_b_pos = grain_a_pos + grain_len` and the engine is bit-transparent
  (modulo the splice search finding an offset of 0 — clamp search to prefer
  zero offset when correlation ties).

### Tones: correlation splice search

`splice_search(target)`: search offsets in ±`SEARCH_MS` (2.5 ms ≈ 110 frames
at 44.1k) maximizing normalized cross-correlation between the outgoing tap's
next `WIN` frames (`WIN` = 64) and the candidate grain's first `WIN` frames:

```text
best = argmax_{d in -S..S} corr(read[grain_a_pos .. +WIN], read[target+d .. +WIN])
```

Cost: 221 offsets × 64 MACs ≈ 14k mul-adds per grain boundary, i.e. every
~40 ms — noise. Do it inline at the boundary frame (audio thread, no alloc,
fixed-size loops). This is what keeps periodic material (bass, vocal) from
doubling/phasing at splice points — it aligns splices to the waveform period,
poor-man's PSOLA.

Tones parameter mapping: `grain_size_ms` 30..100 (default 60), flux locked
to 0, splice search ON.

### Texture: jitter instead of search

`jitter = (rand01()*2-1) * flux * grain_len_src * 0.5` added to `target`;
splice search OFF (random offsets decorrelate the phasiness instead of
fighting it). `grain_size_ms` 8..250 (default 65), `flux` 0..1 (default 0.25).
Big grains + flux = the lush Ableton Texture smear; small grains = robotic.

### Interactions

- `transpose`/`speed` multiply the tap advance rate (native-pitch reads),
  while `ideal_source_frame` uses only the warp ratio — so transpose changes
  pitch without changing timing, exactly like Ableton's warped transpose.
- Loop modes: on loop wrap, reset both taps and grain phase (same hook as
  `reset_forward_warp_state`).
- Reverse: disabled (consistent with Beats).
- The 50 ms `warp_ratio` slew already smooths tempo/bpm-mod changes; grain
  respawn picks up the new ideal position naturally, so **no special ratio-
  jump reseek path is needed** (delete the v0.1 mid-note reseek block for
  these modes; keep it for Beats).

## Mode 4: Complex — phased

Phase vocoder quality without writing a phase vocoder: use
**Signalsmith Stretch** (MIT-licensed C++, `signalsmith-stretch` crate has
Rust bindings; quality sits between Complex and Complex Pro, handles
polyphonic mixes, has built-in transient preservation).

Two integration shapes, in order:

1. **v0.2.x — offline pre-render (recommended first)**: when
   `mode == Complex`, a control-thread job renders the entire warped region
   at the current ratio into a cache keyed by
   `(buffer_id, ratio_quantized, transpose_quantized)`; the voice plays the
   rendered buffer linearly (it IS `ideal_source_frame` materialized). While
   rendering (or after a tempo change), fall back to Re-Pitch and swap in the
   render when ready (with a short xfade at the swap). Zero audio-thread
   risk, perfect quality, cost is memory (ratio-stretched copy per warped
   sample) and a re-render on tempo change. For static-tempo sessions — the
   overwhelmingly common case — this is indistinguishable from realtime
   Complex.
2. **v0.3 — realtime**: per-voice stretcher instances pre-allocated in a
   control-thread pool (the library allocates at configure, processes
   RT-safely), leased to voices at trigger, block-driven from
   `sampler_process`. Adds latency (~grain window, tens of ms) that must be
   compensated at trigger scheduling. Only worth it if tempo automation with
   Complex material becomes a real workflow.

The mode enum reserves value 4 now so projects and UI don't churn.

## Analysis changes

- **Beat grid construction** (`warp_grid.rs`, control thread): as specified
  above. Regenerate on: analysis ready, user BPM edit, ½×/2× press, downbeat
  nudge, preserve change. Publishing follows the existing Arc-swap pattern.
- **Downbeat quality matters more now** (the grid anchors to it). Keep the
  v0.1 heuristic but add the UI nudge: drag the downbeat marker snaps
  `anchor_frame` to the nearest onset or free-drag with shift. Also add a
  "beat-offset" quick control (◀ ▶ buttons shifting the anchor by one 1/16)
  since off-by-one-16th anchors are the most common detection failure.
- No new detectors needed for v0.2. (madmom sidecar remains the follow-up
  for BPM/downbeat accuracy.)

## UI changes (sampler panel)

The warp row grows a mode dropdown and a mode-dependent param cluster,
mirroring Ableton's layout:

```
warp [ON]  mode [Beats ▾]   bpm [174.0] [½×] [2×]   anchor [note ▾]
  Beats:    preserve [1/16 ▾]   loop [Loop ▾]   env  [·——— 0%]
  Tones:    grain [60 ms]
  Texture:  grain [65 ms]   flux [0.25]
  Re-Pitch: (no extra params)
  Complex:  (renders in background — show spinner/RENDERED badge)
```

- Dropdowns reuse the existing loop-mode dropdown widget; knobs the existing
  numeric knob.
- Waveform overlay: draw the **grid boundaries** (from `WarpGridShared`)
  instead of raw onsets when warp is on; onsets remain visible as fainter
  ticks in Preserve=Transients mode. Downbeat marker stays draggable.
- Param writes go through the existing param-edit path; preserve/loop/env/
  grain/flux are plockable like other sampler params (they're plain state
  slots), BPM keeps its mod lanes.

## Scheduler changes

- Compute and send `anchor_offset_frames` when `warp_anchor == transport`
  (one new aux; step→beat conversion uses the pattern's steps-per-beat).
- Send the new aux params (preserve, loop mode, env, grain, flux, grid ptr).
- Ratio is still computed at trigger from `project_bpm / sample_bpm`
  (user-override BPM wins over analysis, unchanged), and still updated
  per-block via the BPM mod-lane path.

## Implementation plan

Ordered for usefulness-per-effort; each step ships independently.

1. **Re-Pitch mode** + mode-enum plumbing (state slot semantics, aux, UI
   dropdown). Small, immediately useful, forces the mode-switch scaffolding
   into place.
2. **WarpGrid** module + construction from bpm/downbeat/preserve + unit tests
   (boundary math vs hand-computed grids at several BPM/sr combos; the
   ratio-inversion trap called out above gets an explicit test:
   174 BPM sample in a 120 BPM project must consume ~0.69 source frames per
   musical frame).
3. **Beats rewrite** on top of WarpGrid: scheduled-boundary timing fix,
   loop-tail fill (Off/Loop/Ping-Pong), segment envelope. Delete the
   audio-thread onset-table read. Smoke-test matrix: amen at project 87/120/
   140/160/174, each preserve setting, each loop mode; a sparse 2-bar break
   (worst case for v0.1) must stay locked.
4. **Preserve=Transients** grid construction (snap/insert/dedupe) — pure
   control-side, reuses step 3's playback verbatim.
5. **Granular engine** (Tones/Texture): taps + window first (Texture with
   flux=0 is the base case), then flux, then the Tones correlation search.
   Verify: 100 Hz sine stretched 0.7×/1.4× shows no pitch change and, with
   splice search on, no amplitude modulation > ~1 dB; a vocal loop at ±30%
   stays intelligible.
6. **Transport anchoring** (aux + wrap math + UI toggle).
7. **Complex pre-render** behind the reserved mode value: cargo dep
   `signalsmith-stretch`, render worker (reuse the analysis worker thread),
   cache + fallback-to-Re-Pitch swap logic.

Steps 1–4 involve no new dependencies and remove the primary complaint
(lossy onset timing). Step 5 makes pitched material work. Steps 6–7 are the
polish that closes most of the remaining gap to Ableton.

## Testing

- Unit tests for grid construction and the segment/grain position equations
  (pure functions, test off-thread).
- Engine-level: extend the existing sampler render harness to render a voice
  with warp enabled at fixed ratios and assert (a) segment boundary output
  times against expected host frames (Beats), (b) autocorrelation-detected
  pitch is ratio-invariant (Tones/Texture), (c) Re-Pitch output equals plain
  resampled reference within interpolation error.
- Ear tests (the real bar): break at ±40% tempo in Beats, bassline in Tones
  at ±20%, pad in Texture at 2× slow, full loop in Re-Pitch — checked at
  each step of the plan.

## Further Steps

- **Warp markers**: replace the uniform grid with a user-editable marker
  table (source_frame ↔ beat_position pairs); `WarpGridShared` becomes
  piecewise-linear and `ideal_source_frame` a lookup — playback code
  unchanged. This is the door to warping rubato material.
- **Time-signature support** for 1-Bar preserve (currently assumes 4/4).
- **Realtime Complex** (stretcher pool, latency compensation).
- **Complex Pro-style formant control** — only via a different library or a
  custom vocoder; revisit after realtime Complex.
- **madmom sidecar** for BPM/downbeat (unchanged from v0.1 spec).
- **Global tempo-change re-render** debouncing for Complex pre-render mode.
