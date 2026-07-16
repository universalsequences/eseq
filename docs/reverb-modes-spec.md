# Multiverb — Multi-Mode Vintage Reverb Builtin (Spec, rev 1)

> **Status:** Phases 1-5 landed. All four tanks are implemented, mode changes
> use the locked 25 ms wet fade-out/clear/25 ms fade-in behavior, host
> modulation targets are wired for decay/size/mod depth/mix, and the custom
> panel includes the four factory settings. Hall depth uses a squared curve so
> ordinary settings breathe subtly (0.15 = 0.675 ms maximum excursion) while
> the top of the knob still reaches the deliberately extreme 30 ms range.

## Why

The existing `Reverb` builtin is Airwindows Galactic (via Sapphire Galaxy) — a Hadamard
feedback-matrix tank tuned for huge ambient washes. It's great at exactly one thing.
This spec adds a **new builtin, "Multiverb"**, with four switchable algorithms modeled on
the classic boxes, all sharing one param set and one panel:

| Mode | Model | Character |
|---|---|---|
| **Plate** | Lexicon-style plate (Dattorro 1997 figure-eight tank) | fast dense attack, smooth bright tail, gently modulated |
| **Hall** | Lexicon 224/480 Concert Hall | low initial density blooming into dense tail, *deep random modulation*, long bass |
| **Quad** | Alesis Quadraverb (the Aphex Xtal reverb) | grainy, metallic-adjacent, dense/smeared 16-bit tails |
| **Mod** | Freeverb with modulated allpasses | familiar comb/allpass verb but the allpass stage is an instrument — deep pitch-wobble chorusing |

The existing `Reverb` stays untouched (projects depend on it). Optionally a later phase
folds Galactic in as a fifth "Galaxy" mode and soft-deprecates the old builtin — decide
after Multiverb ships.

## Research notes (what makes each box sound like itself)

### Lexicon 224 / Concert Hall (basis for **Hall**)
- Short delay lines with lots of feedback: tank round-trip ≤ ~0.1 s, so a 30 s decay is
  ~300 passes through the loop — every artifact accumulates.
- Concert Hall has **low initial echo density that builds over the decay**, and is
  "distinguished by its high degree of modulation" (Costello). Allpass gains run high,
  especially at long decays.
- Modulation is *randomized* (Griesinger): slow random-walk delay-length modulation
  ("spin" = rate, "wander" = depth), large excursions. This is what keeps 70-second
  decays from going metallic, and it detunes the tail audibly — a feature.
- Signature artifacts: linear interpolation quantized to coarse subsample steps
  (~1/32 sample) → a "halo" of noise around the signal; low sample rate → dull top end;
  16-bit fixed-point nodes → soft grit at high feedback. These become the **era** control.
- Decorrelation between L/R output taps (low IACC) is what makes it feel enveloping —
  L and R get *different* tap sets, not a widened mono tank.

### Dattorro plate (basis for **Plate**)
Canonical topology from Dattorro, "Effect Design Part 1" (JAES 1997) — an idealization of
the Lexicon plate programs. All delay values are samples **at 29,761 Hz** — scale by
`fs / 29761` at init:
- predelay → 1-pole bandwidth LP → 4 series input diffusers
  (delays 142, 107, 379, 277; gains 0.75, 0.75, 0.625, 0.625)
- figure-eight tank, two branches cross-coupled; each branch:
  **modulated** decay-diffusion allpass 1 (gain −0.70, delay 672 / 908, ±~12 sample
  excursion, ~0.5–1 Hz) → delay (4453 / 4217) → 1-pole damping LP → ×decay →
  decay-diffusion allpass 2 (gain 0.50, delay 1800 / 2656) → delay (3720 / 3163) →
  feeds the *other* branch input.
- Output = fixed weighted tap sums: 7 taps per channel pulled from both branches'
  delays and allpass internals (use Dattorro's published tap table).

### Alesis Quadraverb (basis for **Quad**)
- Confirmed as *the* Xtal / early-Aphex reverb (reverbmachine SAW analysis: the 808 in
  Xtal is drenched in Quadraverb).
- Topology (per Costello, KVR): **4 allpass loops**, each loop = 2 allpass delays + 1
  plain delay in a feedback loop, and **the end of each loop feeds the beginning of the
  next** — a cascade of loops, denser than parallel. Output taps summed from all loops.
- Original programs: Room / Chamber / Hall / Plate / Reverse, with Decay, Diffusion,
  Density, **Low Frequency Decay, High Frequency Decay**, and a gate (hold / release /
  gated level). We expose LF/HF decay via `bass` and `damp`, diffusion via `diffusion`.
- Character: 1988 16-bit DSP running flat out — dense, smeared, grainy, slightly
  metallic. Essentially unmodulated (deliberately); our `mod depth` still works but
  defaults near 0 in this mode's preset. Modulating **decay/size while it runs** is the
  Aphex move — see mod targets below.
- Gate/Reverse programs are a Phase-6 stretch goal (envelope-shaped multitap, cheap once
  the tank exists).

### Freeverb + modulated allpasses (basis for **Mod**)
Standard Freeverb tunings @44.1k, per channel: 8 parallel lowpass-feedback combs
(1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617; feedback = 0.7 + 0.28·size; `damp` in
the comb feedback path) → 4 series allpasses (556, 441, 341, 225; g = 0.5), right
channel all +23 samples (stereospread). The twist: the four allpasses per channel get
**interpolated, LFO-modulated delay reads** — per-allpass LFOs at detuned rates
(e.g. 0.71/0.93/1.17/1.39 × `mod rate`), depth up to ±48 samples, plus an optional
random-walk component. Shallow = chorusing shimmer; deep = seasick pitch-smeared tails.
This is the max-expressibility mode: nearly every knob modulates cleanly.

## Where it lives

Rust builtin per the established recipe (see `builtin-ott-effect` memory; Compressor is
the minimal template, Space Echo the closest analog since it also has a mode grid and
mod-target slots):

1. `crates/sequencer/src/multiverb.rs` — state consts + `MULTIVERB_STATE_SIZE`,
   `MULTIVERB_PARAM_*`, `init`/`process` extern "C", `multiverb_vtable()`.
2. `src/lib.rs`: `pub mod multiverb;`
3. `src/effects.rs`: `builtin_multiverb()` descriptor, name in `builtin_insert_names()`
   + exact-names test, arm in `builtin_insert()`.
4. `src/ui/effects.rs` `create_builtin_effect_node()`: name → (vtable, state size).
5. Panel `metal-seq-fx/builtin/multiverb.lisp`, loaded in `metal-seq-builtin-fx-ui.lisp`,
   dispatched in `builtin/audio-fx.lisp` (nested-if dispatcher: extra trailing paren per
   branch), file added to `metal_seq_core_lisp_files_parse` list, params fixture in
   `state_values.rs`, layout test.

All four tanks are allocated in one state block (they're small — combined delay memory
≈ 2 s of stereo audio, far under the existing Reverb's 13 buffers); only the active mode
processes, so CPU is one-tank cost. Mode switch crossfades output over ~50 ms and clears
the incoming tank (no stale energy, no click); tails from the old mode are dropped —
acceptable, matches hardware program-change behavior.

## Shared parameter set (descriptor order — append-only forever, plocks depend on it)

| # | name | range | notes |
|---|---|---|---|
| 0 | mode | 0–3 enum | Plate / Hall / Quad / Mod |
| 1 | decay | 0–1 | maps per-mode to loop gain / comb feedback; top of range ≈ freeze |
| 2 | size | 0–1 | scales all tank delay lengths; 0.5 = reference tuning, top half → 2×, bottom half dives to 0.03× (few-ms comb/resonator bank); changing live pitch-smears — intentional |
| 3 | predelay | 0–250 ms | |
| 4 | damp | 0–1 | HF decay (in-loop 1-pole LP) |
| 5 | bass | 0–1 | LF decay multiplier (in-loop low shelf), 0.5 = neutral |
| 6 | diffusion | 0–1 | input diffuser gains (+ loop AP gains in Quad) |
| 7 | mod rate | 0.05–8 Hz | "spin" |
| 8 | mod depth | 0–1 | "wander" — per-mode max excursion (Hall/Mod deep, Plate subtle) |
| 9 | mod shape | 0–1 | blend LFO → random-walk (Hall defaults random, Mod defaults LFO) |
| 10 | era | 0–1 | vintage grain: coarse-quantized interpolation, 16-bit-ish node quantization, bandwidth LP (0 = clean modern, 1 = 1979) |
| 11 | width | 0–1 | stereo tap decorrelation → M/S width |
| 12 | mix | 0–1 | equal-power |
| 13 | enabled | 0/1 | `Self::enabled_param`; original Phase-1 position is locked |
| 14–85 | mod source controls | 4 standard effect modulator slots | LFO / envelope / random / drift / external source controls |
| 86–101 | mod target slots | 4 slots × depth for: decay, size, mod depth, mix | Space-Echo pattern, appended after the source controls |

The mod-target slots are the "he modulates it" feature: sequencer p-locks / LFOs can
drive decay/size per step, which in Quad mode is exactly the Xtal trick.

## Phases

**Phase 1 — skeleton + Plate.** `multiverb.rs` with full state layout, all params, mode
switch scaffold (fade/clear), Dattorro plate implemented and tuned. Registration + basic
panel (knobs only, mode grid stubbed to Plate). Tests: descriptor bounds/name pinning,
bypass passthrough, impulse render produces tail with RT60 monotonic in `decay`, no
NaN/denormal after 30 s of silence (flush denormals in loop nodes).

**Phase 2 — Hall.** 224-style loop: 2 input diffusers → loop of 4 sections
(AP ≈ g 0.6–0.72 → AP → modulated delay), damping LP + bass shelf once per loop, decay
gain distributed per section, L/R output = two disjoint 4-tap sets. Random-walk
modulator: per-delay smoothed sample-and-hold noise, quadrature-ish phase offsets,
excursions up to ~30 ms · mod depth². Tune for the density-bloom (verify echo-density
growth on impulse render) and for staying non-metallic at decay ≈ 0.95 with mod depth
≈ 0.3.

**Phase 3 — Quad.** 4 cascaded allpass loops (2 APs + delay each, loop n feeds loop
n+1, taps from all loops). LF/HF decay shelves in each loop. `era` defaults ~0.6 here:
truncated (non-interpolated) modulated reads + 16-bit quantization of loop feedback.
Reference-check by ear against Xtal / SAW 85-92 and the reverbmachine writeup.

**Phase 4 — Mod (freeverb).** Straight Freeverb port, then swap the 4 series allpasses
per channel for interpolated modulated reads (per-AP detuned LFO rates + random blend
via mod shape). Verify: mod depth 0 nulls against the unmodulated reference render;
depth 1 gives obvious pitch-wobble without zipper noise or buffer overrun (allocate
AP buffers with max-excursion headroom).

**Phase 5 — panel + mod targets + presets.** Mode button grid (space-echo pattern,
`fx-set-effect-value` numeric index), per-mode default-highlight of relevant knobs,
mod-target slots wired (including both BPM-push arms if any param becomes sync-able —
currently none). Factory presets: "Xtal Wash" (Quad, long decay, era 0.7, mod-target on
decay), "224 Bloom" (Hall), "Gold Plate", "Seasick" (Mod, depth 1.0).

**Phase 6 (stretch) — Gate/Reverse programs** for Quad (envelope-shaped multitap +
gate hold/release/gated-level), and the decide-later Galaxy-mode fold-in.

## Landed verification

- Every mode produces a finite, decorrelated tail; late energy is monotonic in
  `decay`; minimum `size` settles into a short resonator in all four modes.
- Hall impulse density blooms by more than the required 1.5×. At depth zero,
  Hall and Mod null exactly when rate/shape continue moving internally.
- The 16 descriptor-indexed host targets affect rendered audio. Routing params
  stay private in the UI while legitimate controls named `mod rate`, `mod depth`,
  and `mod shape` remain visible.
- Factory settings reset all four source selections and all 16 routing depths.
  Xtal Wash then assigns a slow drift source in slot 1 to decay through target
  metadata, so its advertised decay modulation is active immediately.
- The production Metal capture renders the complete custom panel at 2000×520;
  layout tests assert finite, nonzero geometry for its sections, labels, and knobs.

## Tuning & verification workflow

- Offline render bin `src/bin/multiverb_render.rs` (spring_tune pattern): renders
  impulse + click-track + sustained-pad WAVs per mode/param grid into `tuning_out/`.
  Metrics: RT60 via EDC crossings (check −10/−20/−30 dB times directly — geomspace-grid
  EDC error underweights the audible late tail, per spring tuning), echo-density curve
  (Hall must bloom, Plate must start dense), spectrogram for modulation character.
- Ear pass against references: ValhallaVintageVerb demos (Hall/Plate), Xtal stems /
  reverbmachine SAW recreation (Quad), stock Freeverb (Mod null test).
- Unit-test gotchas inherited from Space Echo/OTT: pre-seed smoother state slots in
  short renders; `cargo test -p sequencer --lib multiverb::` to avoid name collisions;
  state slots are renumberable but **descriptor order persists** — append only.

## Locked decisions

- New builtin, existing `Reverb` untouched.
- Rust builtin (not dgenlisp) — per-sample modulated interpolated delay reads in four
  topologies is exactly what the builtin recipe is for, and mode switching inside one
  dgenlisp graph would compile all four tanks into the hot path.
- One shared param set, per-mode interpretation (VintageVerb model) — not per-mode
  param pages.
- `era` is a single macro knob, not separate interpolation/bit/bandwidth params.
- Mode switch drops the old tail (crossfade + clear), no dual-tank ring-out.

## Sources

- https://reverbmachine.com/blog/aphex-twin-selected-ambient-works-85-92/ — Quadraverb on Xtal
- https://valhalladsp.wordpress.com/tag/lexicon-224/ — 224 architecture, halo/interpolation, concert-hall bloom + modulation
- https://www.kvraudio.com/forum/viewtopic.php?t=349039 — Costello on Quadraverb 4-allpass-loop cascade topology
- Dattorro, "Effect Design Part 1: Reverberator and Other Filters," JAES 45(9), 1997 — plate topology + tap tables
- Freeverb (Jezar at Dreampoint) standard tunings
