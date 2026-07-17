# Roar — multi-stage coloring / saturation builtin (spec)

Target: ~90% of Ableton Live 12's **Roar**. A multi-stage waveshaper/distortion
with seven routing topologies (single / serial / parallel / multiband /
mid-side / feedback / delay), 12 shaper curves, a per-stage filter, a global
feedback network, and a one-knob output compressor. We deliberately drop
Roar's internal modulation matrix and sidechain input (see "Out of scope") —
eseq's own 4-slot per-param modulation system covers that ground.

Builtin name: `"Roar"`. Implementation follows the builtin-effect recipe
established by OTT/Compressor/Phaser-Flanger:
`crates/sequencer/src/effects/roar.rs` (vtable + state consts), descriptor in
`src/effects.rs`, arm in `create_builtin_effect_node()`, panel in
`ui/effects/builtin/roar.lisp`, dispatcher in `builtin/audio-fx.lisp`,
loader in `ui/builtin-effects.lisp`.

---

## 1. Signal flow overview

```
in ─▶ Drive ─▶ Tone ─▶ [ stage topology per Routing ] ─▶ Feedback network
                                                       (Feedback/Delay modes)
   ─▶ Compress ─▶ Output gain ─▶ Dry/Wet (equal-power) ─▶ out
```

Everything is stereo throughout. Dry tap is taken at the device input
(pre-Drive), matching Ableton.

### Stages

There are up to **3 identical stages**. Each stage is:

```
stage in ─▶ [Filter if Pre] ─▶ ×amount-drive ─▶ +bias ─▶ shaper ─▶ DC block
         ─▶ [Filter if Post] ─▶ Level gain ─▶ stage out
```

How many stages are active, and what feeds them, depends on Routing.

### Routing modes

| Mode       | Stages | Tabs shown          | Topology |
|------------|--------|---------------------|----------|
| Single     | 1      | Stage 1             | in → S1 → out |
| Serial     | 2      | Stage 1 / Stage 2   | in → S1 → S2 → out |
| Parallel   | 2      | Stage 1 / Stage 2   | in → S1, in → S2; out = crossfade by **Blend** |
| Multi Band | 3      | Low / Mid / High    | LR4 3-band split (crossovers **Low**/**High** fields, defaults 200 Hz / 2.00 kHz); S1=low, S2=mid, S3=high; sum |
| Mid Side   | 2      | Mid / Side          | M/S encode; S1=mid, S2=side; **Blend** = M/S balance; decode |
| Feedback   | 2      | Stage 1 / Stage 2   | S1 in forward path; feedback loop around S1 whose return path runs through S2 → FB bandpass → ±FB Amount, delayed by FB Mode time |
| Sabbath-style short feedback (drones/resonance). |
| Delay      | 2      | Stage 1 / Stage 2   | Same loop but the delay-line output is mixed into the output (audible echoes with saturation in the regeneration path) |

Multiband reuses the LR4 crossover from `ott.rs` (allpass-compensated low
band, flat sum at Amount 0). Mid-side: `m = (l+r)/√2`, `s = (l−r)/√2`,
decode inverse; Blend 50 = unity, toward 0 emphasizes Mid stage output,
toward 100 emphasizes Side.

Parallel Blend: equal-power crossfade of the two stage outputs
(50/50 default, displayed like Ableton as `50 / 50`).

**Fidelity note (verify by ear against Live):** which stage sits in the
Feedback/Delay loop is our best reading of the device; if A/B testing shows
Roar keeps both stages in the forward path with a plain feedback tap, adjust
the topology — the parameter surface doesn't change.

---

## 2. Input section

- **Drive** — input gain into the stage topology, −12…+36 dB, default 0 dB.
- **Tone** — tilt EQ post-Drive, ±100%, default 0%. Positive tilts toward
  highs (+ up to ±6 dB shelves at the extremes), negative toward lows.
  Pivot frequency is the editable **Tone Freq** field (50 Hz–18 kHz, log,
  default 180 Hz). The small curve button toggles Tone response between
  **Tilt** and **Shelf** (single low-shelf cut/boost) — Ableton shows an
  LP-curve icon here; verify exact second mode by ear, tilt is the primary.
- Implementation: one-pole tilt (complementary LP/HP pair recombined with
  ±gain) — cheap and smooth under modulation.

## 3. Stage parameters (×3, tab-selected)

| Param | Range | Default | Notes |
|---|---|---|---|
| Shaper Type | 12 options | Soft Sine | see §4 |
| Amount | 0–100% | 0% | pre-shaper gain, maps 0→1× … 100%→~64× (exp curve). At 0% the stage is transparent (shaper bypassed via dry mix, not just unity gain — keeps null test clean) |
| Bias | −1.00…+1.00 | 0.00 | DC offset added pre-shaper, removed post-shaper (one-pole DC blocker ~10 Hz). Drawn as the dashed vertical line in the shaper view |
| Level | −24…+24 dB | 0 dB | post-stage trim |
| Filter Type | 9 options | LP | see §5 |
| Frequency | 20 Hz–16 kHz (log) | 16 kHz | stage filter cutoff/center |
| Res | 0.00–1.00 | 0.10 | resonance (maps to Q 0.5–~12) |
| Pre | toggle | off | filter placed **pre**-shaper when on, post when off (the `Pre` button beside the Frequency knob) |

### Anti-aliasing

Shapers run at **2× oversampling** (polyphase halfband up/down around the
nonlinearity only). Bit Crusher and Resampling are intentionally aliased and
run at base rate. If CPU allows, gate a 4× "HQ" behind a compile-time const
first; don't expose a param.

## 4. Shaper curves

All curves take pre-gained, biased input `x` and return `y`; every curve is
normalized so the small-signal slope at the origin is ~1 (level stays put as
Amount rises; loudness change comes from saturation, matching Roar's feel).
`a` = Amount 0–1 where a curve needs an extra shape control.

1. **Soft Sine** — `y = sin(clamp(x, −π/2, π/2))`; classic smooth fold-free
   soft clip (the S-curve in the screenshots).
2. **Digital Clip** — `y = clamp(x, −1, 1)`; hard edge.
3. **Bit Crusher** — quantize: `y = round(x·L)/L`, `L = lerp(64, 2, a)`
   levels; no oversampling.
4. **Diode Clipper** — asymmetric exponential knee:
   `y = x − 0.5·(exp(k·max(x−t,0)) − 1)/k` style one-sided soft clamp,
   softer on negative half (Shockley-flavored).
5. **Tube Preamp** — asymmetric soft: `y = tanh(x + 0.2·x²)` with the even
   term DC-blocked; 2nd-harmonic warmth.
6. **Half Wave Rectifier** — `y = max(x, 0)`, then ×2 and DC block.
7. **Full Wave Rectifier** — `y = |x|`, DC blocked (octave-up flavor).
8. **Polynomial** — odd Chebyshev blend: `y = (1−a)·x + a·(3x−4x³)/…`
   clamped; harder odd harmonics as Amount rises.
9. **Fractal** — iterated soft fold: apply `sin`-fold 3× with decaying gain;
   chaotic upper harmonics.
10. **Tri Fold** — triangle wavefolder: reflect `x` back at ±1 repeatedly
    (`y = tri(x)` periodic-fold); classic West-coast fold.
11. **Noise Injection** — `y = softclip(x) + a·n·|x|` where `n` is white
    noise; noise rides the signal envelope, silent at silence.
12. **Shards** — glitch curve: soft clip with `a`-scaled sample-hold
    discontinuities (quantized phase segments); the "broken" one. Lowest
    fidelity bar — anything spiky/glitchy that tracks Amount passes.

Exact matches to Ableton's private curves are impossible; the target is that
each menu entry occupies the same sonic slot. The shaper view (§7) renders
whatever curve the DSP actually uses — keep the curve function `pub` in
`roar.rs` (like `phaser_flanger::notch_frequencies()`) so the widget and
tests share it (dual-maintain gotcha from the wavetable widget applies —
avoid it by exposing one shared fn through node state or a small table).

## 5. Stage filter types

SVF-based where possible (one SVF core, mode-selected outputs):

1. **LP** / 2. **BP** / 3. **HP** / 4. **Notch** / 5. **Peak** — standard
   SVF outputs, Res → Q.
6. **Morph** — continuous LP→BP→HP morph; Res doubles as the morph
   position's Q. (Roar morphs via a separate control we don't have; mapping
   the morph to Frequency-independent fixed blend LP/BP/HP at Res-Q is
   acceptable; simplest: morph position hardcoded mid = BP-leaning. Flag for
   iteration.)
7. **Comb** — feedforward+feedback comb tuned to Frequency, Res = feedback
   amount (positive polarity).
8. **Resampling** — sample-rate reducer: hold interval from Frequency
   (16 kHz = off → 20 Hz = extreme), Res adds regen ringing via a small
   feedback around the hold.
9. **Dispersion** — cascade of 4 first-order allpasses tuned to Frequency,
   Res = allpass coefficient spread; smears transients (chirpy attack).

## 6. Feedback section (right panel, always visible; audible only in Feedback/Delay routing)

| Param | Range | Default | Notes |
|---|---|---|---|
| FB Mode | Time / Note | Time | Note = tempo-synced divisions (reuse `SYNC_BEATS` grid; needs the `push_all_delay_bpm` arms — same gotcha as Phaser-Flanger) |
| Time | 0.5–1000 ms (log) | 18.2 ms | Feedback routing sweet spot is short; Delay routing typically Note mode |
| Note div | 1/32…1 grid | 1/8 | shown when FB Mode = Note |
| Amount | 0–100% | 0% | loop gain, internally clamped ≤ ~0.98 with a soft-limiter (tanh) in the loop so it can scream but not blow up |
| Ø | toggle | off | invert feedback polarity |
| Duck | toggle | off | input envelope ducks feedback return (feedback blooms in the gaps). Ableton shows a small histogram icon here — verify semantics; ducking is our reading |
| Freq \| Width | 30 Hz–18 kHz / 0.5–9.0 oct | 1.00 kHz / 8.00 | bandpass in the feedback path: center freq + bandwidth in octaves (two one-pole HP/LP skirts) |

Loop: `fbline ← delay(stage-return + duck·env, time)`; return path =
S2 (in Feedback/Delay routing) → bandpass → ±amount → summed at S1 input.
Interpolated (Hermite) delay read so Time is modulatable without zipper.

## 7. Output section

- **Compress** — 0–100%, default 0%. One-knob program compressor on the wet
  path: fixed ~4:1, threshold slides down and makeup rises with the knob
  (Amount = macro over thr/makeup), ~5 ms attack / ~120 ms release, peak
  detector.
- **SC HPF** — toggle; 120 Hz highpass on the compressor's detector only
  (bass stops pumping it).
- **Output** — ±24 dB wet trim, default 0.
- **Dry/Wet** — 0–100%, equal-power, default 100%.

## 8. Parameter inventory (descriptor order)

Globals: `drive`, `tone`, `tone freq`, `tone mode`, `routing` (option:
Single/Serial/Parallel/Multi Band/Mid Side/Feedback/Delay), `blend`,
`xover low` (40 Hz–1 kHz, log, default 200), `xover high` (500 Hz–10 kHz,
log, default 2000; clamp `low < high` in process), `fb mode`, `fb time`,
`fb div`, `fb amount`, `fb invert`, `fb duck`, `fb freq`, `fb width`,
`compress`, `sc hpf`, `output`, `dry/wet`.

Per stage, ×3, prefixed `s1 `/`s2 `/`s3 `: `shaper` (option list §4),
`amount`, `bias`, `level`, `filter` (option list §5), `freq`, `res`, `pre`.

Total: 21 + 24 = **45 params**. All plain f32 writes into node state,
clamped in `process` (recipe standard). All continuous params get the usual
4 mod slots + p-locks for free from the descriptor.

State tail reserves live-meter slots (§10): 2 floats pre-shaper level
(min/max of the recent window, for the shaper-view drive region) for the
**selected** stage isn't knowable DSP-side, so store per-stage: 3× {level
dB}, plus 3× post-stage out dB for the H/M/L (or 1/2) mini-meters in the
routing box.

## 9. UI panel (`builtin-fx-roar-ui`)

Layout mirrors the device, left → right:

1. **Input box** — Drive knob (mod-wrapped, dB), Tone knob (%) with tone-mode
   icon button above and Tone Freq number-picker below.
2. **Routing box** — routing dropdown (option buttons or the space-echo-style
   picker), mode-dependent fields underneath: `Low`/`High` crossover fields
   (Multi Band), `Blend 50/50` (Parallel/Mid Side); right edge shows the
   per-stage mini output meters (labels `1/2` or `H/M/L` per mode).
3. **Stage box** (widest) — **tab row** across the top: tab labels and count
   follow routing (Stage 1/Stage 2, Low/Mid/High, Mid/Side). Tab colors:
   stage 1 orange, stage 2 cyan, stage 3 pink/red (reuse
   `phaser-flanger-orange`/`-cyan` + a new pink). Below the tabs, the
   selected stage's controls: Amount knob, Bias knob, Frequency knob +
   `Pre` toggle; then side-by-side the **shaper view** (curve + Shaper
   dropdown + Level field) and the **filter view** (response curve + Filter
   dropdown + Res field). Tab selection is UI-only state (`defstate`
   per-panel), not a param.
4. **Feedback box** — FB Mode dropdown + time/div field, Amount knob,
   Ø + Duck toggle row, Freq|Width stacked number-pickers.
5. **Output box** — Compress knob + SC HPF toggle, Output knob, Dry/Wet.

Knobs use the mod-wrapper pattern keyed `"roar-param-<idx>-mod-wrapper"`
(copy from phaser-flanger.lisp). Option params use
`builtin-fx-set-effect-option`. Fall back to `fx-param-grid` when any param
lookup fails.

### Widgets (2 new, shader-based, display-only)

- **`roar-shaper`** (`crates/eseqlisp/src/widget_render/roar_shaper.rs`) —
  draws the selected stage's transfer curve in the stage color, the dashed
  white bias line at `x = bias`, and a translucent **live drive-region
  overlay**: the horizontal span of curve currently exercised by the input
  (from the per-stage pre-shaper min/max meters — this is the yellow/brown
  band in the screenshots; it widens with Drive/Tone/Amount because those
  raise the level hitting the curve). Props: shaper type, amount, bias,
  stage color, `:source` for the live frame.
- **`roar-filter`** — magnitude response curve for the stage filter
  (type/freq/res props), same log-freq grid style as the space-echo/filter
  panels; check `filter-core.lisp`/`filter-panel.lisp` first — if an
  existing response widget can take type/freq/res props, reuse it and skip
  this widget.

**Registration gotcha (both widgets):** three places in eseqlisp —
`widget_render/mod.rs` (`pub mod` + `WIDGET_DEFINITIONS`) **and**
`src/widgets.rs` `BUILTIN_WIDGET_NAMES`, or lisp eval dies with
`UnknownVariable`.

Pass curve-affecting props as **base-value bindings**
(`instrument-param-base-value`), not snapshot values — knob drags don't
rebuild the panel (phaser-flanger display comment applies verbatim).

## 10. Live meters

Reuse the OTT node-state → widget path:

1. `roar.rs` writes into its state tail: per-stage pre-shaper |peak| dB
   (drive-region overlay) and post-stage out dB (routing-box mini meters),
   5/250 ms ballistics.
2. `live_audio_analyzer.rs`: extend the band-meter request collection to
   recognize the roar widgets' `:source`, publish a frame keyed
   `roar-meter:<source-fragment>` on >0.05 dB movement.
3. Widgets read the frame store in `build_metal_primitives`.

## 11. Implementation order

1. **DSP core** — `roar.rs`: state consts, all 12 shapers + 9 filters as
   free fns, single/serial/parallel first; unit-test shapers (odd/even
   harmonic content, slope-1 normalization, DC-block) via the dgenlisp-style
   C-harness approach or direct Rust tests on `process`.
2. Multiband (lift LR4 from ott.rs; flat-sum test at Amount 0) + Mid-Side
   (mono-compat test: S stage silent input when fed mono).
3. Feedback/Delay network (+ `push_all_delay_bpm` arms for Note mode;
   stability test: 100% Amount, impulse in, output bounded).
4. Registration plumbing (recipe steps 2–4) + `state_values.rs` parse list,
   `test_roar_params()` fixture, exact-names test entry.
5. Panel + widgets + layout test
   (`metal_seq_fx_roar_layout_contains_stage_tabs_and_shaper_display`),
   capture fixture `ui/capture-fixtures/roar-panel.lisp`.
6. Live meter plumbing.
7. Ear-tuning pass against Live 12 Roar with matched material (reuse the
   reference-render workflow from the phaser work — render dry loop, process
   in both, A/B). Tune Amount gain law, Tone slopes, Compress macro curve.

Test-name gotcha: `cargo test -p sequencer roar` will also match anything
containing "roar" — use `--lib roar::`.

## 12. Out of scope (the missing ~10%)

- **Modulation matrix** (Roar's LFO×2 / env follower / noise sources +
  routing panel, the collapsed "Modulation" strip). eseq's per-param 4-slot
  modulation + p-locks covers the musical use; revisit only if internal
  audio-rate mod is missed.
- **Sidechain input** (left rail in Ableton) — only feeds the env follower
  we're not building.
- Exact curve/filter transfer matches — slot-equivalent, ear-tuned.
- Preset bank (add after the panel exists, if wanted).

## 13. Open questions to resolve by ear against Live

- Feedback/Delay routing: is Stage 2 truly in the loop return?
- Tone mode button: what's the second mode (shelf? LP?) and the exact ±100%
  slope.
- The histogram icon in the FB section: ducking, gate, or envelope amount?
- Whether Compress is pre or post the wet Output trim (spec says pre).
