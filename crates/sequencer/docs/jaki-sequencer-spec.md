# Jaki Sequencer — Specification

A Liebezeit-style micro-pattern generative sequencer, implemented as a
**pure-Lisp package** on top of `def-sequencer`. No Rust port. The pattern
language is s-expressions — the Lisp reader is the lexer, a `jaki/pat` macro
is the parser — and patterns are first-class values transformed by pure
functions and fanned out to multiple tracks from a single generator.

This replaces the retired `jaki-midi-fx-spec.md`, which predated
`def-sequencer` and proposed a Rust parser/evaluator exposed through the
midi-fx runtime. Everything Rust in that spec is gone; what survives is the
musical model (velocity dynamics, hand alternation, transforms) re-expressed
as Lisp over the generator substrate.

---

## 1. Goals

- A `jaki` **package**: pattern constructors, pure transforms, and an emit
  helper, loadable as a module (`jaki/pat`, `jaki/emit`, `jaki/filter`, …).
  This is the pilot package for the module/package system (eseq-mods.6) — a
  forcing function: every expressiveness gap Jaki hits is a gap any
  third-party package author would hit.
- **Pattern-as-value.** `(def base (jaki/pat …))` produces an immutable
  pattern value. Transforms (`jaki/shift`, `jaki/rev`, `jaki/filter`) are
  pure functions returning new patterns; derived patterns stay in sync with
  their base by construction.
- **One generator, many tracks.** A single `def-sequencer` body evaluates a
  base pattern and routes slices to different tracks via `seq-emit :track` —
  left hand to one drum, right hand to another, accents to a third.
- **Hands are core.** The Liebezeit hand-alternation model is structure, not
  annotation: it generates the feel and it is the routing axis for
  multi-track output. Full port of the Swift hand semantics (§6).
- The generator free-runs on its own `:resolution` grid. No step gating —
  Jaki is a generative voice, not an event transformer.

## 2. Non-Goals

- **No Rust.** Parser, evaluator, velocity model, hand model — all package
  Lisp. The only engine surface used is what `def-sequencer` already
  provides plus the prerequisites in §9.
- **No string mini-notation, no compatibility with `JAKI_PATTERNS.md`.**
  Clean break, decided deliberately: the s-expr grammar is strictly more
  expressive (time-mod arguments are ordinary expressions) and costs the
  reader nothing. Patterns do not copy-paste from the Swift patch-editor.
  No `(jaki/parse "…")` shim is planned.
- **Not a midi-fx.** The reactive per-step framing in the old spec was a
  contortion around the substrate that existed at the time. (A reactive
  variant could be layered later; out of scope.)
- Source-position metadata and the patch-editor cursor UI.
- Swing transforms (per-track swing already exists) and per-event ADSR
  overrides (no `seq-emit` story for them yet).

## 3. Behavioral Reference: the Swift Implementation

`~/code/swift/patch-editor` remains the **behavioral** reference — for what
the music should do, never for architecture:

- `Sources/Jaki/JakiEvaluator.swift` — velocity state machine (lines 3–24),
  hand derivation (`deriveHands`, line 135), hand filters
  (`filterByHand` line 1079, `filterByAccent` line 1104), filter guards
  (lines 179–211), hand-scoped transforms (`applyHandSpecificTransform`,
  line 1125), figure evaluation and state threading (`evaluatePiece`,
  line 667; ending hand at 813–818), alignment padding
  (`generateAlignmentPadding`, line 70).
- `Sources/Jaki/JakiTypes.swift` — the transform/time-mod vocabulary.
- `Tests/EngineTests/Jaki*.swift` — test semantics to port (§10).

The Swift parser (`JakiParser.swift`, 331 lines) has no counterpart here:
the reader plus a macro replace it entirely.

## 4. Pattern Language

### 4.1 Shape-based grammar

`jaki/pat` is a **macro** — its body is unevaluated data. Events are bare
symbols; everything list-shaped is a transform or time-mod. No separator
token is needed (the string notation's `|` exists only because a flat token
stream has no structure; parens already provide it — and `|` is a reserved
Pipe token in the eseqlisp lexer anyway).

```lisp
(jaki/pat . . -
  (every 2 swap)
  (every 4 rev)
  (* (cyc 1 2 3 4)))
```

Lexer facts this relies on (verified against `eseqlisp/src/lang/parser.rs`):
`.` and `-` are legal bare symbols (`.` is only numeric when a digit
follows; a bare `-` has an explicit lexer test). Because the macro never
evaluates them, they shadow nothing — do **not** `(def . …)`.

**Events:**
- `.` — dot: one hit, one unit.
- `-` — dash: two hits on one unit boundary pair (the second at +1 unit),
  played by one hand (§6). Two units long.

### 4.2 Figures

A single-figure pattern lists events directly (§4.1). Multi-figure patterns
wrap each figure in `fig`, with per-figure transforms and time-mods inside:

```lisp
(jaki/pat
  (fig (. . -) (* 2))
  (fig (. -)   (/ 3) (every 2 (stac))))
```

The macro disambiguates by shape: a body starting with event symbols is one
implicit figure; a body of `fig` lists is a concatenation. Figures share
velocity state and hand state across their boundaries (§5, §6), matching
Swift `evaluatePiece` / `endingVelocityState` / `endingHand`.

### 4.3 Time-mods

Spelled as operator lists with **ordinary expression arguments**:

- `(* n)` — fast: n cycles of the figure in the space of one.
- `(/ n)` — slow.
- `(% n)` — fit: squash/stretch the figure to exactly n units.

`n` is any expression evaluated per cycle: a literal (`(* 2)`), a
cycle-alternation (`(* (cyc 1 2 3 4))`), a param read (`(* (param-get
"density")))`, once §9.1 lands). Note the spelling is `(* 2)` with a space —
a glued `*2` lexes as one opaque symbol and the Lisp cannot extract its
digits (no string→number native; see §9.3 for the optional sugar).

### 4.4 Cycle alternation

`<a b c>` from the string notation becomes `(cyc a b c)`: a plain function
returning `(nth vals (mod cycle (len vals)))` for the current cycle index.
No special evaluator plumbing (Swift's `CycleFloat` threading dissolves —
the generator already evaluates the pattern per cycle with the index in
hand). Usable anywhere a number is expected: time-mods, `every` counts,
velocity overrides.

### 4.5 Transforms (in-figure or whole-pattern)

Adopted from the Swift vocabulary; each is a list form:

- `(rev)` — reverse event order. Hands re-derive after (§6.2).
- `(rot n)` — rotate by n positions.
- `(trunc n)` — drop the last n symbols.
- `(every n <transform>)` — apply on every nth cycle; nests freely,
  including around hand forms.
- `(stac)` — staccato: gate = 0.25 units.
- `(ghost)` — skip a dash's first hit, keep the second as a pickup.
- `(split <target>)` / `(merge <target>)` — dash↔dot-pair rewrites.
- `(swap)` — exchange hand assignment (L↔R) for this cycle.
- `(basevel v)`, `(dotdecay v)`, `(dashdecay v)`, `(minvel v)`,
  `(maxvel v)` — velocity-model overrides; `v` is any expression, including
  `(cyc …)`.
- `(L <transform>)` / `(R <transform>)` — hand-scoped transform (§6.4).
- `(align n)` / `(align n :pad)` — the `@N` / `@N-` alignment forms:
  pad the figure to n units; `:pad` fills with ghost events whose hands and
  velocity state thread through (Swift `generateAlignmentPadding`).

### 4.6 Whole-pattern transforms as functions

Every in-pattern transform also exists as a pure function over a pattern
value, so derived patterns can be built outside `jaki/pat`:

```lisp
(def base (jaki/pat . . - (every 2 rev)))

(jaki/rev base)
(jaki/shift base 1)                    ; rotate right by n units
(jaki/every base 4 jaki/rev)
(jaki/filter base :hand :left)         ; §7
(jaki/for-hand base :left (jaki/stac)) ; hand-scoped, function form
```

`jaki/pat` desugars its trailing transform lists onto exactly these
functions — the macro is thin; the functions are the implementation.

## 5. Velocity Model

Port of `JakiVelocityParams` / `JakiVelocityState` (Swift lines 3–24):
parameters `base`, `dot-decay`, `dash-decay`, `accent-boost`, `min-vel`,
`max-vel`; state `current`, `prev-was-dash`, `dot-streak`. The state
threads through the figure fold, across `fig` boundaries and alignment
padding, and — because the generator free-runs — **across cycles** via the
generator's persistent state (no per-step reset; the old spec's restart
semantics existed only because a midi-fx dies with its step). A
`(jaki/reset)` helper and a reset-on-pattern-edit are the escape hatches.

An event following a dash is **accented** (velocity boosted by
`accent-boost`); `(accent)`-filtered views key off this flag (§7).

## 6. Hand Model

Reverses the old spec's "drum-kit specific, not useful" cut — that judgment
was an artifact of one-fx-one-track. Hands are the routing axis and the
engine of the style. Full port:

### 6.1 Derivation (`deriveHands`, Swift line 135)

Walk the event list with a current hand (start: left). Each **dot** takes
the current hand; each **dash** takes the current hand for *both* of its
hits (one hand bouncing — the physical gesture); after every event the hand
toggles. `. . - .` → L, R, L+L, R.

### 6.2 Ordering rule

Hands are derived **after** event-order transforms (Swift line 706 derives
from `transformedEvents`). `rev`/`rot`/`trunc` produce a fresh alternation
over the resulting sequence — hands describe how a drummer would play the
result, they are not glued to events. The Lisp evaluator must apply
order transforms first, then derive hands. This is a correctness
invariant with dedicated tests (§10).

### 6.3 Threading

Each figure ends with an ending hand: even event count keeps the starting
hand, odd toggles it (Swift 813–818). The next figure and any alignment
padding start from there, so alternation is continuous across
concatenation.

### 6.4 Hand-scoped transforms

`(L <t>)` / `(R <t>)` apply `<t>` only to that hand's events, including
nested `(every n …)`. Function form: `(jaki/for-hand pat :left t)`.
`(swap)` exchanges the assignment wholesale.

### 6.5 What does not carry over

The Swift both-hands-cancel guard (`shouldIgnoreHandFilters`, line 208) and
the `every`-scoped filter-activation bookkeeping (179–201) existed because
string-notation filters mutated the single output stream in place. Here
filters are non-destructive derivations from a shared base (§7), so
`(jaki/filter base :hand :left)` and `(jaki/filter base :hand :right)` are
two independent values and nothing needs to cancel. These guards are
dropped, deliberately.

## 7. Filters

`(jaki/filter pat <axis> <value> …)` — keyed, varargs-conjunctive:

```lisp
(jaki/filter base :hand :left)
(jaki/filter base :symbol :dash)
(jaki/filter base :hand :left :symbol :dash)
(jaki/filter base :accent true)
(jaki/filter base :figure 1)
```

Axes are keys on the evaluated event: `:hand` (`:left`/`:right`),
`:symbol` (`:dot`/`:dash`), `:accent` (bool), `:hit` (1 or 2, which dash
hit), `:figure` (index, for splitting concatenations across tracks).

**Gate extension** (the musical part, from Swift):
- `:hand` filter — surviving events keep their offsets; each gate extends
  legato to the next same-hand event, the last to cycle end
  (`filterByHand`, line 1079).
- `:accent` filter — gates extend to the next *unfiltered* event
  (`filterByAccent`, line 1104).
- Other axes — gates unchanged (default); an optional `:legato true` key
  opts into hand-style extension.

## 8. Runtime Model

### 8.1 Generator integration

One `def-sequencer` per Jaki instance:

```lisp
(import jaki)

(def-sequencer "jaki-kit"
  :resolution :16
  :init (jaki/init)
  :tick
  (let ((base (jaki/pat . . -
                (every 2 swap)
                (* (cyc 1 2)))))
    (do
      (jaki/emit :track 0 base)
      (jaki/emit :track 1 (jaki/shift base 1))
      (jaki/emit :track 2 (jaki/filter base :hand :left)))))
```

- **1 unit = 1 tick of the generator's `:resolution` grid.** No step gate,
  no hard cut, no units-available arithmetic — the old spec's §5 dissolves.
- **Cycle index** is a pure function of `(gen-tick)`, so the generator is
  deterministic and survives transport relocation the same way neural does.
  With a constant cycle length it is `(floor (/ (gen-tick) cycle-length))`.
  When a time-mod varies the length per cycle (`(% (cyc 4 6))`), lengths
  repeat with the `cyc` period P and super-cycle length L = Σ lengths, so
  cycle = `P·floor(pos/L)` plus a scan of the partial sums — still closed
  form, no accumulated state.
- `jaki/emit` evaluates the pattern for the current cycle (memoized, §8.2),
  selects events whose `unit-offset` falls in `[pos, pos+1)`, and calls
  `seq-emit` per event with `:track`, `:at` (`:now` or the fractional
  offset via `gen-offset`), `:vel` (model velocity × instance level),
  `:dur` (gate in units × unit beats). `:quantize`, `:chord`, `:pan`,
  `:speed` pass through as caller keys on `jaki/emit`.
- Sub-unit placements (a dash's second hit, ghost pickups) are emitted from
  the boundary tick that owns them with a fractional `:at` — the engine's
  lookahead queue does the sample math, per the lisp-sequencer spec's
  timing contract.

### 8.2 Evaluation and memoization

`jaki/pat` expands at macro time into a pattern **value** (a dict: figures,
transforms, and a closure-per-cycle evaluator). Per-cycle evaluation output
(the timed event list) is memoized in generator state keyed by
`(pattern-identity, cycle-mod)` — the old spec's Rust LRU becomes a
two-line Lisp memo. Editing the pattern (hot reload of the `def-sequencer`
body) naturally rebuilds everything.

### 8.3 Subdivisions and rational time

`(% n)` fit produces events at rational offsets — `(. . . -)%4` scales five
units of content by 4/5, landing hits at 0, 4/5, 8/5, 12/5, 16/5 units.
This is fully supported by the existing substrate because of one property:
**`:resolution` is a query cadence, not a placement grid.** `seq-emit :at`
takes an arbitrary beats number with no snapping (quantize is opt-in), so a
tick that owns window `[pos, pos+1)` emits any event in that window at its
exact fractional offset; the engine's lookahead queue does the sample math.
Nothing about tuplets, N-in-M fits, or stacked-density polyrhythms
(Tidal-style) requires engine changes — they all reduce to events at
rational offsets within a cycle.

Two implementation rules:

- **Exact rational offsets in the evaluator.** Offsets and gates are
  normalized `(num . den)` pairs (integer math in f64 is exact to 2^53);
  conversion to float beats happens only at the `seq-emit` call. Window
  membership `[pos, pos+1)` is then an exact integer comparison — no
  epsilon, no double-fired or dropped hit when a tuplet lands on a window
  edge. Pure Lisp; no natives needed.
- Tuplet events must not pass `:quantize` (default is already off).

### 8.4 Pitch

Jaki produces rhythm; pitch policy is layered:

- **v1 — params**: `root`, `scale`, and a small degree-walk (`direction`
  enum as in arp) as sequencer params; `jaki/emit` maps hand/accent/figure
  to degrees. Self-contained, works day one (after §9.1).
- **v2 — seeding**: subscribe to a track per the lisp-sequencer spec's
  seeding section, so punched-in steps supply the notes Jaki rhythmicizes.
  This restores the "compose with chords/arp" story and is the most musical
  option; deferred only because seeding is engine work.

## 9. Prerequisites and Language Gaps

1. **`param` contract for `def-sequencer`** (`lisp-sequencer-remaining.md`
   §1) — declared params with a `String` kind (the pattern source when
   edited from UI), `(param-get name)` in `:tick`, per-pattern
   serialization. This supersedes the old spec's string-`midi-fx-param`
   prerequisite. Without it Jaki is code-edited only — which is an
   acceptable v1.
2. **Text-input widget** for editing pattern source from the UI panel.
   Deferrable for the same reason.
3. **Optional, not blocking — `parse-num` native** (string → number, nil on
   failure): would allow glued `*2` sugar by cracking the symbol's text.
   The canonical spelling `(* 2)` needs nothing. File under
   package-expressiveness follow-ups (macros can inspect symbol identity
   but not symbol content).

## 10. Test Plan

Pure-function core means the evaluator tests are plain Lisp-level tests
(pattern in, event list out), runnable through the existing scheduler-VM
test harness. Port the *semantics* of the Swift suites:

- `JakiTests.swift` — grammar shapes, transforms, velocity model.
- `JakiStepTimingTests.swift` — unit offsets and gate durations.
- `JakiAlignmentTests.swift` — `(align n)` / `(align n :pad)` padding,
  hand/velocity threading through pads.
- `JakiTimeVaryingCycleTest.swift` — `(cyc …)` in time-mods across cycles.
- `JakiTripletTests.swift` — triplet `:resolution` mapping.
- Hand-routing cases — **un-skipped** from the old plan: derivation
  (dot-alternate, dash-same-hand), §6.2 derive-after-transform, ending-hand
  threading, `(L …)`/`(R …)` scoping, filter gate-extension.

New integration tests (generator-level):

1. `(. . . .)` at `:16` free-runs: cycle index advances every 4 ticks,
   `(cyc a b)` alternates per cycle, velocity state threads across the
   boundary (no per-step reset).
2. Dash sub-hit lands at the correct fractional `:at`; same hand both hits.
3. Three `jaki/emit` calls from one tick route to three tracks; the
   `:hand :left` view's gates extend legato per §7.
4. `(every 2 swap)` — hands exchange on even cycles only.
5. Hot-reload of the pattern rebuilds the memo and restarts cleanly.
6. `(. . . -) (% 4)` — five hits at exactly 0, 4/5, 8/5, 12/5, 16/5 units;
   each hit emitted exactly once across tick windows (rational membership,
   §8.3), including a fit whose event lands exactly on a window boundary.
7. `(% (cyc 4 6))` — cycle index tracks the variable-length super-cycle
   (§8.1 closed form) and stays correct after transport relocation.

## 11. Implementation Phases

1. **Evaluator core** (pure Lisp, no generator): event fold, velocity
   model, hand derivation + threading, transforms, `cyc`, figures,
   alignment. Lisp-level tests.
2. **`jaki/pat` macro + filter/transform function surface** (§4.6, §7).
3. **Generator wiring**: `jaki/init` / `jaki/emit`, memoization,
   fractional-offset emission. Integration tests.
4. **Package-ification**: module header, load path, `import jaki` — track
   alongside eseq-mods.6; Jaki is its pilot content.
5. **Params + UI panel** (after §9.1): pattern string param, level, root /
   scale / direction; small event-dot visualization across the cycle.
6. **Seeded pitch** (v2, §8.3) once generator seeding lands.

## 12. Open Questions

- **`fig` transform scope**: whether a whole-pattern transform after `fig`
  lists applies per-figure or to the concatenation (Swift applies global
  transforms to the concatenation — lean the same way; per-figure is
  already expressible inside each `fig`).
- **`swap` vs `rev` interaction**: `swap` exchanges derived hands; `rev`
  re-derives. Order of application within one transform list is
  left-to-right — confirm against Swift fixtures when porting tests.
- **Velocity params as instance state vs pattern transforms**: both exist
  (`basevel` transform and a `level` param). Precedence: transform wins
  within its scope. Revisit if confusing in practice.
- **Polymeter between emit sites**: `(jaki/shift base 1)` on another track
  already yields phase offsets; a `(jaki/scale pat r)` time-stretch per
  emit site would give true polymeter — cheap to add, defer until wanted.
