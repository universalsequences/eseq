# Filterbank — Sherman Filterbank 2 style dual-filter mangler (spec)

Target: the musical core of the **Sherman Filterbank 2** — a fuzzy analog
input stage into two harmonically-linked multimode filters with audio-rate
self-FM, self-ring-mod, a bipolar ADSR, an audio-rate LFO, and an output AR
envelope. The signature nastiness comes from the hardware's LTC1060
**switched-capacitor** filter chips: a sampled SVF whose internal clock
tracks the cutoff, leaking stepping/aliasing/clock-bleed into the audio at
low cutoffs. We model that clock explicitly (§4) — it is the part every
"Sherman-inspired" plugin skips and the reason none of them sound like one.

We deliberately drop the pitch tracker, MIDI note output, LINK chaining, and
pedal I/O (see "Out of scope"). We add things the hardware only offers via
patch cables: sidechain FM/AM sources and a stereo split mode.

Builtin name: `"Filterbank"`. Implementation follows the builtin-effect
recipe established by OTT/Compressor/Phaser-Flanger/Space Echo:
`crates/sequencer/src/effects/filterbank.rs` (vtable + state consts),
descriptor in `src/effects/mod.rs`, arm in `create_builtin_effect_node()`,
panel in `ui/effects/builtin/filterbank.lisp`, dispatcher in
`builtin/audio-fx.lisp`, loader in `ui/builtin-effects.lisp`.

Reference material: Sound On Sound FB2 Compact review (signal path),
AmbientSpace "Shermanik" Bitwig recreation notes (topology + normalled
routings), LTC1060 datasheet (clock:cutoff ratios 50:1 / 100:1).

---

## 1. Signal flow overview

```
in ─▶ Input drive ─▶ Hi EQ ─▶ ┌ Filter 1 ┐ serial↔parallel ─▶ AM/Ring ─▶ AR env
                              └ Filter 2 ┘      morph                  ─▶ Dry/Wet ─▶ out
        ▲ FM (audio-rate, both cutoffs)   ▲ modulator = filter 2 out (default)
        │ default source = input itself   │ or sidechain
        └ or sidechain

mod sources → cutoffs: ADSR/env-follower (per-filter amount), LFO (± depth),
              FM input, harmonics link (F2 slaved to F1 ratio)
noise + feedback: summed into the filter input, post-drive
```

Dry tap is taken at the device input (pre-drive). Mono-in hardware; we run
stereo-linked by default (identical filter pair per channel, shared
envelopes/LFO detected from the mono sum), with a **Stereo Split** toggle
(§6) that instead sends filter 1 → L and filter 2 → R.

## 2. Input section

- **Input** — drive into the filters, −12…+30 dB, default 0 dB. Unity at
  knob center like the hardware; above unity applies progressive asymmetric
  saturation *before* the filters (this is why everything through a Sherman
  sounds "already hairy"). Use `roar::shaper_transfer` Tube/Diode-flavored
  curve at 2× oversampling around the nonlinearity (space-echo record-head
  pattern, `space_echo.rs:725`).
- **Hi EQ** — 3-way option Cut / Flat / Boost: ±6 dB high shelf (~3 kHz) in
  the drive stage, matching the hardware's amplifier HF switch.
- **Sense** — envelope-trigger threshold, 0–100%. The input's own envelope
  (post-drive) crossing Sense fires the ADSR and resets the LFO (if LFO Trig
  on). Hysteresis ~3 dB + 20 ms retrigger holdoff so sustained material
  doesn't machine-gun the envelope.
- **Noise** — 0–100%: white noise summed into the filter input (deterministic
  LCG, `roar.rs:289 next_noise`). **Feedback** — 0–100%: output fed back to
  the filter input through a fixed soft-limit (tanh) and a one-pole HP
  (~30 Hz DC guard), internally clamped ≤ ~0.95 loop gain. Hardware only
  enables these with no input jack; we expose them always — Filterbank as a
  drone box on an empty track is a feature.

## 3. The two filters

Both filters are identical 12 dB/oct (2-pole) multimode SVFs — which is
exactly what the LTC1060 is internally. Per filter:

| Param | Range | Default | Notes |
|---|---|---|---|
| Freq | 20 Hz–16 kHz (log) | 500 Hz | cutoff before modulation |
| Res | 0–110% | 20% | ≥100% self-oscillates; see resonance law below |
| Mode | 0–100% | 0% | continuous LP → BP → HP morph: weighted blend of the SVF's simultaneous lp/bp/hp outputs (0 = LP, 50 = BP, 100 = HP), roar-stage style (`roar.rs:492`) |

Shared filter controls:

- **Correction** — 0–100%, default 0: subtracts phase-inverted BP from each
  filter's morphed output. Mid values steepen the effective slope / carve a
  notch at cutoff; at max in LP/HP positions it approaches notch, and can
  null the output entirely at BP position — that's authentic, don't clamp it
  away. (Hardware has one correction control; if A/B says it's per-filter,
  split the param — surface doesn't change otherwise.)
- **Ser/Par** — 0–100% continuous morph, default 100 (parallel). 0 = serial
  (F1 → F2), 100 = parallel (F1 + F2 averaged), between = crossfade of the
  two topologies (compute both paths; F2 runs once, fed by the crossfaded
  blend of input and F1 output).
- **Harmonics** — option: `Free, 1, 1.5, 2, 3, 4, 5, 6, 8, 9, 12, 16`,
  default Free. Non-Free slaves filter 2's cutoff to **F1 freq ÷ ratio**
  (SOS: "a quint down (1.5)", "an octave down (2)") — F2's own Freq knob is
  ignored while linked; its Res/Mode stay live. All modulation (env/LFO/FM)
  applies to F1 *before* the division so linked sweeps stay harmonic.

### Resonance law

The Sherman's resonance is the "gets really wild" part: it self-oscillates
hard and interacts with the input saturation. Implementation: SVF damping
`k = 2 − 2·(res/100)` clamped to a small positive floor never reached until
res > 100%, **tanh soft-limit on the bp state inside the loop** (keeps
screaming bounded, adds the compressed "pummelled" character under drive),
and Res Bleed (§7) lets the envelope lean on resonance. Self-oscillation
pitch must track Freq accurately — harness-verifiable.

## 4. Switched-capacitor clock model (the Sherman part)

Each filter's SVF core does **not** run per host sample. It runs at an
internal clock `f_clk = ratio × f_c` (LTC1060 style), with zero-order hold
between updates:

- Per host sample: `clk_phase += f_clk / sr`; while `clk_phase ≥ 1`,
  subtract 1 and run one SVF update on the current (held) input; output
  holds between updates. When `f_clk ≥ sr` this degenerates to the normal
  per-sample SVF — no CPU or fidelity cost at high cutoffs. Floor `f_clk`
  at ~200 Hz so it never stalls.
- Because `f_c = f_clk / ratio` by construction, the SVF `g` is a
  **constant** `tan(π / ratio)` — cutoff modulation moves the clock rate,
  not the coefficient. This is the switched-cap topology verbatim, and it's
  what produces the visible waveform stepping and inharmonic aliasing at low
  cutoffs for free.
- **Crunch** — 0–100%, default 25%: morphs `ratio` 100:1 → 25:1 (log) and
  scales a clock-bleed term: a small square-ish tone at `f_clk` (± its
  aliases, which the ZOH generates naturally) summed into the filter output,
  level keyed to Crunch and rising as `f_clk` drops into the audible band.
  At Crunch 0 the clock model is still active but polite (100:1, no bleed) —
  fully "clean" is not on the menu, this is a Sherman.

- **Continuous HP feedthrough** (fidelity fix from break-testing): only the
  lp/bp integrator outputs are ZOH-held — the hp output is recomputed every
  host sample as `x − k·bp_held − lp_held` from the **live** input. On the
  LTC1060 the HP node is a continuous-time op-amp sum (input arrives through
  resistors; only integrator contributions are sampled). Holding the input
  too resamples the whole program at f_clk — HP mode at 20 Hz turned entire
  breaks into a 2 kHz bitcrush. Regression test
  `hp_at_low_cutoff_passes_live_input_unheld`.

No oversampling around the filters — the aliasing is the instrument. The
input-drive nonlinearity keeps its 2× (§2) so the *drive* stays creamy while
the *filters* crunch.

## 5. Modulation

### FM
- **FM Amount** — 0–100%, exponential depth up to ±4 octaves of per-sample
  cutoff modulation on **both** filters (pre-harmonics-link).
- **FM Source** — track selector, `HostControl::FxSidechain
  { input_channel: 6 }` (ports 2..5 are the ext-mod inputs, §5a). Unset =
  normalled to the **post-drive input signal** (self-FM, the hardware
  default). Audio-rate: applied per sample to `f_clk` — the ZOH clock makes
  this gnarly rather than smooth, which is correct.

### ADSR → cutoffs
| Param | Range | Default | Notes |
|---|---|---|---|
| ADSR/AR mode | ADSR / Follower | ADSR | Follower replaces the ADSR shape with the input envelope (attack/release from A and R knobs, ~`filter.rs` env-follow pattern) |
| Attack | 0.5 ms–4 s (log) | 5 ms | |
| Decay | 1 ms–4 s (log) | 200 ms | |
| Sustain | −100…+100% | 0% | **bipolar** — negative sustain yanks the cutoff below rest, the classic Sherman duck-then-bloom |
| Release | 1 ms–8 s (log) | 300 ms | |
| Env→F1 / Env→F2 | −100…+100% | +50 / +50 | per-filter attenuverters, exponential (±4 oct full scale) |
| Res Bleed | 0–100% | 10% | envelope leaks into both resonances (Shermanik measured ~10% default on hardware) |

Trigger: input threshold via Sense (§2). Params are p-lockable like
everything else, so per-step envelope variation comes free.

### LFO
| Param | Range | Default | Notes |
|---|---|---|---|
| LFO Rate | 0.01 Hz–2 kHz (log) | 0.5 Hz | audio-rate top end is the point; no tempo sync in v1 (the ranges where sync matters are covered by p-locking Rate) |
| LFO Wave | Sine / Saw / Ramp / Square | Saw | reuse `filter.rs:91 lfo_value` shapes |
| LFO Depth | −100…+100% | 0% | **negative depth inverts the modulation on filter 2 only** — filters sweep in opposition (the hardware's stereo trick; devastating with Stereo Split) |
| LFO Trig | toggle | off | Sense trigger resets LFO phase |

### AM / Ring
- **AM Depth** — 0–100%, default 0. 0–50 = amplitude modulation depth,
  50–100 = crossfade into full ring mod (Shermanik-verified hardware law).
- **AM Source** — track selector (`FxSidechain { input_channel: 7 }`).
  Unset = normalled to **filter 2's output** (self-ring-mod: the output
  modulating itself through the harmonics link is where the bell-metal
  clang lives).

### 5a. Host modulation targets (ext1–4 / LFO mod slots)

This device is a modulation magnet — being able to throw the host's ext1–4
/ LFO sources at it is a first-class requirement, not descriptor
boilerplate. House mechanics (Space Echo / Roar / Phaser-Flanger pattern):
the four mod sources arrive as **audio-rate inputs on ports 2..5**
(`app/effects.rs` wires ext slot *n* → port `2 + n − 1`), and each mod
**target** param carries 4 contiguous depth state slots
(`STATE_MOD_<T>_DEPTH_1..4`, exported as `FILTERBANK_PARAM_MOD_<T>_DEPTH_*`
for the mod-wrapper UI). Targets and how each applies, summed per sample
across the 4 slots as `Σ depth_i · mod_i`:

| Target | Application | Why |
|---|---|---|
| `f1 freq` | exponential, ±4 oct full depth, pre-harmonics-link (linked F2 follows — harmonic sweeps stay harmonic) | THE sweep |
| `f2 freq` | exponential, ±4 oct; ignored while Harmonics is linked | independent dual-sweep in Free mode |
| `f1 res` / `f2 res` | linear add into res %, clamped 0–110 | push a filter in and out of self-osc rhythmically |
| `f1 mode` / `f2 mode` | linear add into morph %, clamped 0–100 | LP→HP morph sweeps, very Sherman |
| `fm amount` | linear, clamped 0–100 | gate the self-FM chaos in and out |
| `am depth` | linear, clamped 0–100 | ride AM→ring — tremolo that decays into clang |
| `ser/par` | linear, clamped 0–100 | topology as a performance gesture |
| `crunch` | linear, clamped 0–100 | modulated fidelity — clock grit as rhythm |

10 targets × 4 = 40 depth slots. Mod inputs are full audio-rate buffers, so
all applications are per-sample (freq targets need no smoothing — the
clocked SVF eats steps by design; the linear targets get the standard
one-pole ~2 ms lag to avoid zippering, matching space-echo).

**Second wave (added after break-testing)** — 9 performance-control targets:

| Target | Application | Why |
|---|---|---|
| `sense` | linear add, block rate | dynamics-reactive triggering |
| `attack` / `decay` / `release` | exponential time scale ±3 oct, block rate | envelope shapes riding a sidechain |
| `sustain` | linear add ±100, block rate | duck-depth as a gesture |
| `lfo rate` | exponential ±3 oct, block rate; **ignored while synced** (p-lock the division instead) | sweep-speed rides |
| `lfo depth` | linear ±100, per-sample (2 ms lag) | wobble amount as a fader |
| `ar attack` / `ar release` | exponential ±3 oct, block rate | gate shape morphing |

Block rate = the mod sources sampled once per audio block (1–10 ms); these
params feed per-block envelope/threshold coefficients, so per-sample
application would buy nothing audible. Total: 19 targets × 4 = 76 depth
slots. Everything still p-lock-only: input, hi eq, noise, feedback,
correction, env f1/f2, res bleed, ar depth, output, dry/wet.

## 6. Output section

- **AR Out** — Attack 0.5 ms–2 s / Release 1 ms–4 s / Depth 0–100%: an AR
  envelope (triggered by Sense, same gate as the ADSR) applied to the wet
  **amplitude**. Depth 0 = bypass. This is the hardware's gating/pumping
  envelope — pseudo-sidechain pumps, chopped stabs.
- **Stereo Split** — toggle, default off: wet L = filter 1 path only, wet
  R = filter 2 path only (forces parallel topology while on; Ser/Par knob
  ignored). With negative LFO depth or Free harmonics this is a huge
  stereoizer. Off = stereo-linked processing.
- **Output** — ±24 dB wet trim, default 0 (the hardware famously has no
  output level; we're not that authentic).
- **Dry/Wet** — 0–100%, equal-power, default 100%.

## 7. Parameter inventory (descriptor order)

Globals: `input`, `hi eq` (option Cut/Flat/Boost), `sense`, `noise`,
`feedback`, `crunch`, `correction`, `ser/par`, `harmonics` (option, 12
entries), `fm amount`, `fm source` (FxSidechain), `am depth`, `am source`
(FxSidechain), `env mode` (option ADSR/Follower), `attack`, `decay`,
`sustain`, `release`, `env f1`, `env f2`, `res bleed`, `lfo rate`,
`lfo wave` (option ×4), `lfo depth`, `lfo trig`, `ar attack`, `ar release`,
`ar depth`, `stereo split`, `output`, `dry/wet`.

Per filter, ×2, prefixed `f1 `/`f2 `: `freq`, `res`, `mode`.

Total: 31 + 6 = **37 knob params**, plus 40 mod-depth params for the 10
targets in §5a (space-echo naming: `FILTERBANK_PARAM_MOD_<T>_DEPTH_1..4`).
All plain f32 writes into node state, clamped in `process` (recipe
standard); every continuous param gets p-locks free from the descriptor,
and the §5a targets additionally take ext1–4/LFO mod sources. Internal
audio-rate territory (self-FM, LFO at kHz) lives *inside* the node; the
ext-mod inputs are themselves audio-rate buffers on ports 2..5, so external
audio-rate modulation of the §5a targets also works.

Node inputs: 0/1 = L/R, 2..5 = ext-mod sources, 6 = FM sidechain, 7 = AM
sidechain.

State tail reserves live-meter slots (§9): input-drive level dB, env value
(bipolar), gate flag, F1/F2 effective cutoff Hz (post all modulation) — the
last two drive the response display.

## 8. UI panel (`builtin-fx-filterbank-ui`)

Layout mirrors the hardware left → right; Sherman-yellow accent color
(the panel is famously school-bus yellow — lean in):

1. **Input box** — Input knob (dB), Hi EQ 3-way, Sense knob with a small
   gate LED (env meter), Noise + Feedback mini-knobs.
2. **Filter 1 / Filter 2 boxes** — per filter: Freq (large), Res, Mode
   morph knob with LP/BP/HP tick labels. Between them: Harmonics dropdown
   (the hardware's biggest knob — give it visual weight), Correction,
   Ser/Par, Crunch.
3. **Modulation box** — env-mode toggle + ADSR row (A/D/S/R), Env→F1/F2 +
   Res Bleed attenuverter mini-knobs; LFO row: Rate/Wave/Depth/Trig; FM
   Amount + source picker; AM Depth + source picker (source pickers reuse
   the compressor sidechain track-selector control).
4. **Output box** — AR A/R/Depth, Stereo Split toggle, Output, Dry/Wet.

Knobs use the mod-wrapper pattern keyed
`"filterbank-param-<idx>-mod-wrapper"` (copy `space-echo.lisp:13`). Option
params via `builtin-fx-set-effect-option`. Fall back to `fx-param-grid`
when any param lookup fails.

### Display widget (1 new, optional — cut if it drags)

**`filterbank-display`** — dual magnitude-response curves (F1 yellow, F2
white) on the shared log-freq grid style of the filter/space-echo panels,
positioned from the *live* effective-cutoff meter slots so envelope/LFO/FM
sweeps are visible, with a faint vertical comb marking `f_clk` when it's
below ~8 kHz (you can see the crunch coming). Check
`filter-core.lisp`/`filter-panel.lisp` first — if the existing response
widget takes freq/res/mode props, reuse it for a static version and defer
the live one. Registration gotcha applies: `widget_render/mod.rs`
(`pub mod` + `WIDGET_DEFINITIONS`) **and** `src/widgets.rs`
`BUILTIN_WIDGET_NAMES`. Pass curve props as base-value bindings, not
snapshot values (phaser-flanger comment applies verbatim).

## 9. Live meters

Reuse the OTT node-state → widget path: `filterbank.rs` writes gate/env/
effective-cutoffs into its state tail (5/250 ms ballistics where they're
levels), `live_audio_analyzer.rs` publishes a frame keyed
`filterbank-meter:<source-fragment>` on movement, widgets read the frame
store in `build_metal_primitives`. Gate LED + display sweep only — no
multi-band meter walls.

## 10. Implementation order

1. **DSP core** — `filterbank.rs`: state consts; the clocked SVF as a free
   fn (`svf_clocked_tick`) unit-tested directly: (a) at `f_clk ≥ sr`
   matches `filter.rs::svf_sample` within epsilon, (b) at low cutoff the
   output shows ZOH plateaus, (c) self-oscillation frequency tracks Freq
   within a few cents at res 105%. Then drive → dual filter → ser/par →
   correction, static params only.
2. Modulation: ADSR + Sense trigger (test: burst in → gate fires once,
   holdoff respected), follower mode, LFO (audio-rate: sideband test), FM
   from input, harmonics link (linked sweep stays at ratio).
3. AM/ring (depth-law crossfade test: 100% depth + sine modulator =
   suppressed carrier), AR out env, noise + feedback (stability: feedback
   100%, impulse in, bounded out), stereo split.
4. Host mod targets (§5a) — depth slots + descriptor mod entries + port
   2..5 reads; test per space-echo's
   `modulation_inputs_affect_intensity_and_volumes` (`space_echo.rs:1566`):
   DC on a mod port with depth set moves each target, freq targets
   exponentially. Then sidechain FM/AM sources — descriptor `FxSidechain`
   arms + the `app/effects.rs` rewire path (compressor precedent,
   `effects.rs:2076`) on ports **6/7**; verify the effect-node input-port
   allocation handles 8 inputs (compressor stops at 3, mod-users stop at
   6 — first builtin to need both).
5. Registration plumbing (descriptor, dispatch arm, `state_values.rs` parse
   list, `test_filterbank_params()` fixture, exact-names test entry).
6. Panel + layout test
   (`metal_seq_fx_filterbank_layout_contains_dual_filters_and_harmonics`),
   capture fixture `ui/capture-fixtures/filterbank-panel.lisp`; display
   widget last.
7. **Ear-tuning pass** against reference recordings (no hardware on hand:
   use published FB2 demo material — SOS audio examples, YouTube runs of
   known dry loops). Tune: drive curve asymmetry, resonance law/self-osc
   onset, Crunch bleed level vs cutoff, AM→ring crossfade feel. The
   C-harness/audition workflow applies for the measurable half (sweep
   renders, FM sideband spectra, clock-stepping scope shots).

Test-name gotcha: `cargo test -p sequencer --lib filterbank::`, and scoped
tests only per the house test workflow.

## 11. Out of scope

- **Pitch tracker** (F2 follows detected input pitch): the hardware's own
  tracker is famously flaky; harmonics ratios + p-locking Freq cover the
  musical intent. Revisit only if missed — crude autocorrelation would slot
  in as a 13th Harmonics option ("Track").
- **MIDI note output / triggering, LINK chaining, pedal I/O** — host-level
  concepts that don't map to a builtin; Sense + p-locks replace them.
- ~~**Tempo-synced LFO**~~ — BUILT: `lfo sync` toggle + `lfo div` dropdown
  (shown in place of the rate knob while synced; same 11-division table as
  Str8 Delay/Space Echo/Phaser-Flanger, `push_all_delay_bpm` arm added).
  Descriptor params sit in the LFO group; state slots (167..169) sit past
  the bypass-reset span so bypass doesn't clear them. NOTE for future param
  additions once projects exist in the wild: saved-project defaults restore
  positionally, so new params must then be tail-appended, not inserted
  mid-list.
- Exact LTC1060 noise floor / component-level modeling — clock model +
  bleed is the character target, not a circuit sim.

## 12. Open questions (resolve by ear / A-B against recordings)

- Correction: single shared knob (as specced) or per-filter on the FB2?
- Exact harmonics ratio list — SOS says "1, 1.5, 2, 3, 4…16" (11 ratios);
  confirm the middle entries against the FB2 manual before freezing the
  option list.
- Does the hardware's FM hit both filters equally, or F1 with F2 following
  through the link? (Specced: both, pre-link.)
- Real clock:cutoff ratio in the Sherman — LTC1060 supports 50:1/100:1;
  Crunch's 100→25 range assumes we want *more* grit available than stock.
- Whether input drive is pre or post the Hi EQ switch (specced: drive
  first).
