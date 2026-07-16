# Jaki MIDI FX — Specification

A Liebezeit-style micro-pattern MIDI effect for the sequencer, ported from the
Swift `Jaki` module in `~/code/swift/patch-editor`. Implemented as a thin Lisp
wrapper over a Rust-side parser/evaluator that exposes precomputed event lists
to the existing `midi-fx` runtime.

---

## 1. Goals

- A new MIDI effect `jaki` that, given a pattern string like `(. . -)*2 | accent`,
  expands an incoming step into a burst of timed sub-events with Liebezeit-style
  velocity dynamics, accents, and gate overrides.
- Composable with existing chord / scale / arp infrastructure: Jaki produces
  rhythm; pitch comes from `(fx-notes)`.
- Stable timing: the pattern's tempo is set by the track timebase (or an
  explicit `rate` param), **not** by the step's duration. Step duration acts as
  a higher-level *gate* on the pattern, hard-cutting events past the gate.
- Parser/evaluator lives in Rust (testable, allocation-free at runtime). Lisp
  is a small wrapper, similar in size to `midi-fx/arp/dsp.lisp`.

## 2. Non-Goals (initial scope)

- Hand routing (`left`/`right`/`L:`/`R:` transforms) — Liebezeit hand model is
  drum-kit specific; not useful for a melodic burst. Defer or drop.
- Polyphony of Jaki output to multiple tracks. One Jaki fx → one track lane.
- Source-position metadata (`JakiSourcePosition`, `sourceCharStart/End` in
  `JakiTimedEvent`) — only needed for the patch-editor cursor UI, not us.
- `swing`/`swingres` transforms — sequencer already has per-track swing; let
  that handle it.
- ADSR override transforms (`attack`, `decay`, `sustain`, `release`) — defer
  until we have a story for per-event envelope override in `fx-emit`.

## 3. Reference: Swift Implementation

Files at `/Users/alecresende/code/swift/patch-editor/`:

| File | Lines | Purpose |
|------|-------|---------|
| `Sources/Jaki/JakiTypes.swift` | 228 | `JakiEvent`, `JakiPiece`, `JakiPat`, `JakiTransform`, `JakiTimeMod`, `CycleFloat`, `JakiAlignment`, `SplitMergeTarget`, `JakiTopLevelOp`. The AST. |
| `Sources/Jaki/JakiParser.swift` | 331 | Recursive-descent parser. Entry: `JakiParser.parse(_:)` (line 327). Returns `JakiPat`. |
| `Sources/Jaki/JakiEvent.swift` | 101 | `JakiTimedEvent` (output struct) and `JakiEvaluatedPattern` (events + cycleLength + figureCount). |
| `Sources/Jaki/JakiEvaluator.swift` | 1400 | Pure evaluator. `JakiVelocityParams`/`JakiVelocityState` at the top (lines 3–24). Key internals: `evaluatePiece` (line 667), `applyGlobalTransforms` (line 905), `applyTransforms` (line 539), `extractVelocityOverrides` (line 458). |
| `Sources/Jaki/JakiTimeTransform.swift` | 51 | `%n`/`*n`/`/n` resolution. |
| `Sources/Engine/operators/core/JakiControl.swift` | — | Control-rate operator using the evaluator. The integration pattern (parse on store, evaluate per tick) is the model for our Rust-side `MidiFxJakiState`. |
| `docs/JAKI_PATTERNS.md` | — | User-facing grammar reference. **Authoritative for the language surface.** |
| `Tests/EngineTests/JakiTests.swift`, `JakiStepTimingTests.swift`, `JakiAlignmentTests.swift`, `JakiControlTests.swift`, `JakiTripletTests.swift`, `JakiTimeVaryingCycleTest.swift` | — | Test fixtures. Port the cases that cover scope (§4 below); skip hand/ADSR/swing cases. |

## 4. Language Surface (in-scope grammar)

Adopt verbatim from `docs/JAKI_PATTERNS.md` *except* hand/ADSR/swing features.

**Events:**
- `.` dot (1 unit)
- `-` dash (2 units, emits two sub-triggers)

**Pieces:** `( events ( | transforms )? )` optionally followed by a time-mod and/or alignment.

**Concatenation:** `(…)(…)(…)` — figures in sequence, sharing velocity state across boundaries (Swift: `evaluatePiece` in `JakiEvaluator.swift:667`, ending state in `JakiEvaluatedPattern.endingVelocityState`).

**Time mods (post-piece):** `%n` (fit), `*n` (fast), `/n` (slow). Cycle-alternating values supported: `*<2 3>`, `%<4 6 8>`. Spec: `JakiTimeMod` in `JakiTypes.swift:43-62`.

**Transforms (in scope):**
- `rev` — reverse event order
- `rot n` — rotate by n positions
- `trunc n` — drop last n symbols
- `every n <transform>` — apply on every nth cycle
- `staccato` / `stac` — gate = 0.25
- `ghost` / `gh` — skip first dash hit, keep second as pickup
- `accent` / `acc` — keep only post-dash-accented events
- `split` / `merge` (with `SplitMergeTarget` from `JakiTypes.swift:87-92`)
- `basevel <…>`, `dotdecay <…>`, `dashdecay <…>`, `minvel <…>`, `maxvel <…>` — velocity model overrides with cycle notation

**Cycle alternation** (`<a b c>`): authoritative spec in `CycleFloat` (`JakiTypes.swift:65-84`) and `JakiTimeMod.value(forCycle:)` (line 54).

**Alignment** (`@N`, `@N-`): see `JakiAlignment` in `JakiTypes.swift:28-39` and `generateAlignmentPadding` in `JakiEvaluator.swift:70`. Include in v1 — small implementation, clarifies timing.

**Velocity model:** Liebezeit dynamics. Parameters `base`, `dotDecay`, `dashDecay`, `accentBoost`, `minVelocity`, `maxVelocity` — see `JakiVelocityParams` (`JakiEvaluator.swift:3`). State machine in `JakiVelocityState` (`JakiEvaluator.swift:15`): `current`, `prevWasDash`, `dotStreak`.

**Out of scope (v1):** `left`, `right`, `L:`/`R:` hand wrappers; `attack`/`decay`/`sustain`/`release`; `swing`/`swingres`.

## 5. Timing Model

The pattern's internal unit (1 dot = 1 unit) is mapped to a track-level musical
duration. **Step duration never compresses the pattern.**

**Unit mapping:**
- Default: 1 unit = one tick of the track's timebase (e.g. timebase `16` → 1 unit = 1/16th note).
- Override: `rate` param (matching arp's enum: `1`, `1/2`, `1/4`, `1/8`, `1/16`, `1/32`, `1/64`, plus triplet variants — see `midi-fx/arp/dsp.lisp:1-6`).

**Gate by step duration:**
- Let `step_beats = fx-source-time` (the gating duration in beats).
- Let `unit_beats = fx-time(rate)`.
- `units_available = floor(step_beats / unit_beats)` (use a small epsilon for FP).
- Emit only events with `sixteenthOffset * unit_beats < step_beats` (strictly less; **hard cut** at the boundary).
- **Hard cut** confirmed: any event whose start lies past the gate is dropped. In-flight events whose start is inside the gate but whose `gateDuration` would extend past the boundary have their effective duration clamped to `step_beats - emit_offset_beats`.

**Looping:**
- If the pattern's natural length in units (`JakiEvaluatedPattern.cycleLength`) is less than `units_available`, loop the pattern. Each loop iteration is a new "cycle" for cycle-alternation (`<a b c>`) resolution and velocity state inherits from the previous cycle's `endingVelocityState`.

**Restart semantics:** cycle 0 of cycle-alternation begins at each step trigger
(not free-running across steps). Velocity state also resets per step trigger.
This keeps Jaki feeling like a "burst per step", not a global generator. (If we
later want sticky state, expose via `fx-state-get`/`fx-state-set` —
`lisp_host.rs:3517-3531`.)

## 6. Rust-Side API

### 6.1 Module layout

New module `src/jaki/`:

```
src/jaki/
  mod.rs        // public surface: parse, evaluate, types
  types.rs      // JakiEvent, JakiPiece, JakiPat, JakiTransform, JakiTimeMod, CycleFloat, JakiAlignment
  parser.rs     // port of JakiParser.swift
  evaluator.rs  // port of JakiEvaluator.swift (in-scope subset)
  event.rs      // JakiTimedEvent, JakiEvaluatedPattern
```

Each Rust file is a near 1:1 port of its Swift counterpart; the file paths above map directly to the references in §3.

### 6.2 Public Rust API

```rust
pub fn parse(input: &str) -> Result<JakiPat, JakiParseError>;
pub fn evaluate(pat: &JakiPat, cycle: usize, velocity_state: JakiVelocityState)
    -> JakiEvaluatedPattern;

pub struct JakiTimedEvent {
    pub unit_offset: f64,        // was sixteenthOffset; renamed since unit ≠ 16th here
    pub symbol: u8,              // 0 = dot, 1 = dash
    pub velocity: f32,
    pub figure_index: u32,
    pub gate_duration: f64,      // in units
    pub is_accented: bool,
    pub dash_hit_number: Option<u8>,
}

pub struct JakiEvaluatedPattern {
    pub events: Vec<JakiTimedEvent>,
    pub cycle_length: f64,           // in units
    pub figure_count: u32,
    pub ending_velocity_state: JakiVelocityState,
}
```

Drop hand/segment/source-position fields from `JakiTimedEvent` (compare `JakiEvent.swift:3-32`).

### 6.3 Parse cache

Parsing a pattern string is pure. Keep a small LRU keyed by pattern string,
shared across all `jaki` fx instances. Entry stores `JakiPat`. Eviction size:
64 entries (generous; patterns are tiny). Misses parse synchronously on the
control thread (parser is fast and called only on `onStore`, never per-tick).

Per-instance, cache the **evaluated** result keyed by `(pattern_id, cycle)`
since cycle-alternating values mean each cycle is a different event list. Size:
8 cycle results per fx instance (enough for `<a b c d>` plus a couple loops).

### 6.4 New Lisp native

Register one native in `src/lisp_host.rs` next to the existing `fx-*` family
(see `lisp_host.rs:3373-3513`):

```
(fx-jaki-events pattern-string cycle-index)
  → list of maps: ({:offset f :vel f :gate f :accent bool :symbol n :figure n :hit n} …)
    Returns nil on parse error (and logs to stderr like other fx natives).
```

`offset` and `gate` are in **beats**, already multiplied by `unit_beats` for
the current track's `rate` param. The Lisp side never sees raw units — this
keeps the wrapper trivial and lets us change unit mapping policy in Rust
without touching effect code.

The native reads the current fx context's `rate` param via the same mechanism
as `fx-arp-count`/`fx-arp-emit` (see `lisp_host.rs:3389-3409` and
`eval_arp_count_current_event` / `eval_arp_emit_current_event`). If `rate` is
not declared as a param, default to `:16`.

Optionally add `(fx-jaki-cycle-length pattern-string cycle-index)` returning
beats — useful for the Lisp wrapper to decide loop boundaries.

### 6.5 Velocity state across cycles

Because we restart per step (§5), the Lisp wrapper passes `cycle-index = 0, 1, 2…`
for each successive *loop within the gate*, not a global counter. Rust seeds
cycle 0 with `JakiVelocityState::default()` and threads `ending_velocity_state`
from cycle N into the input of cycle N+1 via the per-instance cache.

## 7. Lisp Wrapper

`midi-fx/jaki/dsp.lisp` — modeled on `midi-fx/arp/dsp.lisp`:

```lisp
(midi-fx-param "rate"
  :default 4
  :min 0
  :max 12
  :enum "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T")

(midi-fx-param "pattern" :string "(. . -)")
(midi-fx-param "velocity" :default 0.80 :min 0.00 :max 1.00)
(midi-fx-param "octave"   :default 1    :min 1    :max 4)
(midi-fx-param "direction" :default 0   :min 0    :max 3
  :enum "up" "down" "up-down" "random")

(def-midi-fx "jaki"
  (let ((notes        (fx-notes-octaves (fx-notes) (fx-param "octave")))
        (pattern      (fx-param "pattern"))
        (gate-beats   (fx-source-time))
        (basevel      (fx-param "velocity"))
        (direction    (fx-param "direction")))
    (do
      (fx-suppress)
      ;; Loop pattern across gate; jaki-loop-events returns concatenated
      ;; cycles trimmed to gate-beats with hard cut + clamped tails.
      (for-each |i ev|
        (let ((idx (fx-directed-index i (len notes) direction)))
          (fx-emit :beats (get ev :offset)
                   :note  (get (nth notes idx) :note)
                   :vel   (* basevel (get ev :vel))
                   :dur   (get ev :gate)))
        (jaki-loop-events pattern gate-beats)))))
```

Two helpers needed in `midi-fx/_lib/dsp.lisp`:

- `jaki-loop-events pattern gate-beats` — calls `fx-jaki-events` for cycles
  0, 1, … offsetting by accumulated `fx-jaki-cycle-length`, stops once next
  event's offset ≥ gate-beats, clamps any straddling `:gate` to the boundary.
- `fx-directed-index` already exists (`midi-fx/_lib/dsp.lisp:38-49`) — reused
  for note selection (up/down/up-down/random), giving us the chord/arp walk
  the user wants composed with Jaki rhythm.

**String params.** `(midi-fx-param "pattern" :string "(. . -)")` is hypothetical
— current `midi-fx-param` may not support string-typed params. Check
`ui/effects.lisp` and the `midi-fx-param` native registration in
`src/lisp_host.rs`. If unsupported, **add string param support first** —
without it, the effect is unusable. This is a prerequisite, called out in §10.

## 8. UI

A `ui.lisp` modeled on `midi-fx/arp/ui.lisp`. The pattern param needs a text
input field rather than a numeric stepper. If the existing midi-fx UI
framework only supports numeric/enum params today, this is a second
prerequisite. Display the parsed cycle-length and a small visualization (event
dots laid out across the gate) once string params land — a nice-to-have for v1.

## 9. Test Plan

Port these Swift test files to `src/jaki/tests/`:

- `JakiTests.swift` — core grammar, transforms, velocity model
- `JakiStepTimingTests.swift` — `sixteenthOffset` / `gateDuration` numerics
- `JakiAlignmentTests.swift` — `@N` / `@N-`
- `JakiTimeVaryingCycleTest.swift` — cycle-alternating `*<2 3>` etc.
- `JakiTripletTests.swift` — triplet timing (relevant once we map units to triplet rates)

Skip:
- Hand-routing tests (`L:` / `R:` / `left` / `right`)
- ADSR override tests
- Swing tests
- Source-position cursor tests in `JakiControlTests.swift`

Add new integration tests:
1. Pattern `(. . . . . . . . . . . . . . . .)` with `rate=:16`, step duration of
   8 sixteenths → exactly 8 events emitted, last event's `offset + gate` ≤ gate
   boundary.
2. Pattern `(. . . .)` with step duration of 8 sixteenths and `rate=:16` →
   pattern loops twice, cycle index advances for `<a b>` alternation, velocity
   state threads across loop boundary.
3. Pattern with `*<2 3>` — first loop has density 2, second has density 3.
4. Step duration shorter than first event's offset → zero events emitted (hard cut).
5. `(- -)` with a step duration that lands mid-dash → second dash hit dropped;
   the surviving dash's `gate_duration` clamped to the boundary.

## 10. Prerequisites

Both must land before the effect is usable:

1. **String-typed `midi-fx-param`.** Current params (see `arp/dsp.lisp`) are
   all numeric/enum. Need to extend the param-declaration native in
   `src/lisp_host.rs` and the param storage in the midi-fx state to carry
   strings, plumb through `(fx-param "name")`. Touchpoints: `eval_midi_fx_param`
   (`lisp_host.rs:3446-3450`), midi-fx slot state, and the UI param picker.
2. **UI text-input widget for params.** Pattern strings are edited
   character-by-character; the current numeric stepper UI doesn't work. Could
   defer by allowing pattern edits only from a config file or REPL initially,
   but a TUI text field is the right long-term answer.

## 11. Implementation Phases

1. **Rust port** — `src/jaki/` modules. Parser + evaluator + types. Unit
   tests ported from Swift. No effect wiring yet.
2. **String params + `fx-jaki-events` native.** Prereq §10.1 plus new
   native registered in `lisp_host.rs`.
3. **Lisp wrapper** — `midi-fx/jaki/dsp.lisp` + `_lib` helpers. Integration
   tests from §9.
4. **UI** — text input widget; `midi-fx/jaki/ui.lisp`. Visualizer optional.
5. **Compose with chord/scale ideas** — extend the wrapper (or sibling
   `jaki-chord` fx) to walk `(fx-notes)` per sub-event with degree accumulator
   and inversion, completing the `scale.chord` story this spec grew out of.

## 12. Open Questions

- **`rate` vs track timebase for unit.** Spec uses an explicit `rate` param
  (matches arp). Alternative: derive from track timebase, no param. Param
  gives users an extra knob; track-timebase keeps Jaki visually in lockstep
  with the track grid. Recommend `rate` param defaulting to the track's
  timebase resolution — but that's not currently a thing arp does, so opening
  it for discussion.
- **Cycle counter reset.** Spec resets per step trigger. Alternative: free-run
  across the track's lifetime via `fx-state-get`/`fx-state-set`. Could be a
  param (`reset = "step" | "play" | "manual"`).
- **Whether to ship hand transforms.** Cheap to port and gives us free
  ping-pong / L-R splits if we later route to two tracks. Lean: skip in v1,
  reconsider once multi-track-output for midi-fx exists.
