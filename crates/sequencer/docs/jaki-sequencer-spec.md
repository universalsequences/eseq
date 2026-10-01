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
- `(half)` — Liebezeit halftime, the inverse of `(* 2)`, dashes first: each
  `-` claims the `. .` right before it and contracts to one dash, then the
  dots left in each run pair up left to right into single dots, all played at
  twice their length, so the figure keeps its length and its accent count
  (every dash still ends in a dash). `. . . -` plays `.` `-`(×2);
  `. . . . . -` plays `.`(×2) `.` `-`(×2); symbols that cannot contract pass
  through (`. - .`). Hands derive per symbol as usual: a `. . -` contraction
  keeps the hand after the figure, a `. .` one flips it, and claiming dots
  for dashes first keeps those flips to a minimum. `(* 2)` after `(half)` expands the
  halftime symbols back: `. . -` `(half)` `(* 2)` plays `. . -`. Contrast
  `(/ n)` / `slow`, which keeps the symbols and stretches the figure n-fold.
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

Defaults (2026-09-30): `base` 0.8, `dot-decay` 0.5, `dash-decay` 0.5,
`accent-boost` 1.15, `min-vel` 0, `max-vel` 1. The Swift port's 0.85 / 0.9 /
0.3 kept unaccented hits too close to the accents; halving per hit with no
floor makes the accent structure audible. `(dotdecay v)` / `(dashdecay v)` /
`(minvel v)` restore anything else per route.

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

## 7.1 Scoped words: `(on SEL word…)`

A filter *keeps* the matching events and drops the rest. Most musical
intent is different: "on the left hand, lower the velocity; leave everything
else alone". `on` is that: it applies route words **only to the events or
figures a selector matches**, and every other event passes through
untouched.

```lisp
(on left (vel+ -0.1))                    ; left hand quieter, right hand as is
(on (fig 2) (every 2 (fast 2)))          ; figure 2 double time, every other cycle
(on (and (fig 3) tail) (vel* 0.6))       ; figure 3's ghost hit quieter
(on (and (fig 2) dot) (note+ 7) stac)    ; dots in figure 2 up a fifth, short
(on (nth 2 (fig 2)) (fast 2))            ; every other TIME figure 2 plays
```

`for-hand`, `every-fig` and `(every n <post word>)` are special cases of
this one form (`(for-hand :left stac)` is `(on left stac)`, `(every 4 stac)`
is `(on (every 4) stac)`); they keep working unchanged.

### 7.1.1 Selectors

A selector is a predicate over an event's tags. The tags come from the
figure grammar, so nothing has to be annotated by hand.

| Selector | Matches | Level |
|---|---|---|
| `left` `right` | the hit's hand (§6) | event |
| `dot` `dash` | the hit's symbol | event |
| `head` `tail` | first / second hit of a dash (`tail` is the ghost hit) | event |
| `accent` | Liebezeit accents (§5) | event |
| `any` | everything | figure |
| `(fig n)` | authored figure `n`, 1-based, every repetition of it | figure |
| `(rep n)` | the `n`th repetition (1-based) of a figure's `(rep k)` | figure |
| `(every n)` | whole cycles, the `(every n …)` convention: `(c + 1) mod n = 0` | figure |
| `(nth n S)` | every `n`th **occurrence** of the figures `S` picks, counted across cycles; `(nth n)` counts every figure | figure |
| `(and s…)` `(or s…)` `(not s)` | combinations | the lowest of its parts |
| `(s1 s2 …)` / `(cyc s1 s2 …)` | one selector per cycle, like word alternation | the lowest of its parts |

`(fig n)` counts the figures as authored: `(fig (. -) (rep 4)) (fig (. . -))`
has two, the second is `(fig 2)`. The repetitions are distinguished by
`(rep n)`: `(and (fig 1) (rep 4))` is the last `. -`. Numeric arguments are
per-cycle argument data (`(fig (1 2))` alternates figures per cycle).

`(nth n S)` counts occurrences, not cycles. `S` is a static figure selector
(`fig` / `rep` / `and` / `or` / `not`); with `K` figures matching `S` per
cycle, figure occurrence `k` of cycle `c` is occurrence `c·K + k`, picked when
`(c·K + k + 1) mod n = 0`. This is closed-form, so it memoizes and relocates
exactly like everything else; Tidal, whose patterns are stateless functions of
cycle time, has no equivalent (`every` counts cycles only).

### 7.1.2 Words inside `on`

Words split by what they touch, and that decides which selectors may scope
them:

- **Event words** work on one event at a time and take any selector:
  `(vel* s)`, `(vel+ d)` (velocity multiply / add, clamped to 0..1),
  `(note+ n)` (transpose added on top of the route's `(note …)`), `(vel s)`
  (= `vel*` inside `on`), `(note n)` (sets the note of the matched events),
  `(plock NAME v)` (a per-hit parameter lock, below), and every post-op word: `stac`, `(gate s)`, `(shift n)`, `(quant tb)`,
  `left`/`right`/`accent` (filter within the matched events), `rest`/`none`
  (drop the matched events).
- **Structural words** rebuild a figure and need a **figure-level** selector:
  `fast` `slow` `rev` `rot` `trunc` `swap` `ghost` `split` `merge` and the
  velocity-model words (`basevel`, `dotdecay`, …). `(on left (fast 2))` has no
  meaning — half a figure cannot be retimed — so the word is ignored, and the
  row editor never offers it.
- `(every n w)` inside `on` narrows the selector: `(on S (every n w))` is
  `(on (and S (every n)) w)`. Likewise nested `(on S2 w)` is
  `(on (and S S2) w)`.

`(vel* s)`, `(vel+ d)` and `(note+ n)` are also plain route words: at the top
level they apply to the whole route, and `(every n (vel+ -0.2))` gates them.

`(plock NAME v)` locks one parameter for each hit it applies to
(docs/jaki-plock-spec.md §4). `NAME` is a macro-editor label **string** —
`"instrument:cutoff"`, `"fx2:filterbank:freq"`, `"rack-macro:macro_1"`,
`"step-param:pan"` — resolved by name on the track the hit lands on (a name
the destination lacks is skipped there); `v` is a per-hit value in that
parameter's own units. It works as a route word, inside `on` and under
`every`:

```lisp
-> 0 (plock "instrument:cutoff" (seq :hit 100 5535 34 535))
-> 1 (plock "fx2:filterbank:freq" (0.2 0.8))        ; one value per cycle
-> 2 (on left (plock "rack-macro:macro_1" 0.9))     ; left hits only
```

A later `plock` of the same `NAME` on the same hit wins. Labels are checked
once, when the route is built (host native `param-label?`): one that does
not parse (a typo, a `process:` inlet, a non-string) makes that word a
no-op, so the note still plays. `plock` never changes cycle length, so the
length-only evaluator ignores it. Evaluated events carry the locks as a flat
`:params` list (`label value …`) that `emit` hands to `seq-emit :params`.

### 7.1.3 Evaluation

- Event-level `on` is a post op `(:on sel post)`: the matched events form a
  sub-result, the post op runs on it, and the untouched events are merged
  back in offset order. It sits in authored word order with the other post
  ops.
- Figure-level `on` with a structural word attaches `(:on sel xf fig-ctx)` to
  every figure's transform list (and `(:when :on sel fig-ctx …)` to its
  time-mod for `fast`/`slow`), where `fig-ctx` is the figure's expanded index,
  authored index and repetition. The figure evaluates the word only while the
  selector holds.
- Events carry `:afig` / `:arep` (authored figure and repetition, 0-based)
  next to `:fig` (expanded index), plus `:nadd` / `:nset` note adjustments
  that `emit` adds to the route's `(note …)`.
- Selectors contribute to the evaluation period (§8.2): `(every n)` and
  `(nth n …)` by `n`, `(fig n)` / `(rep n)` by their argument's period.

## 7.2 Value sequences: `(seq :clock v…)`

A list of values steps through its members; the **clock** says what moves
it on. Today's lists move once per cycle, which is right for slow harmonic
movement and far too slow for a melody.

```lisp
(note (seq :hit 0 3 7 10))        ; next value on every hit: a melody
(note (seq :fig 0 5))             ; next value on every figure played
(note (seq :cycle 0 5 9 14))      ; next value every cycle
(note (seq :span 0 3 7 10))       ; spread over the cycle: a hit takes the
                                  ; value at its position (Tidal's "0 3 7 10")
(note (0 5 9 14))                 ; sugar for (seq :cycle 0 5 9 14)
```

`(cyc v…)` stays a synonym of `(seq :cycle v…)`.

**Where the clocks step.** `:hit`, `:fig` and `:span` step in **per-hit
values** — the route's `(note …)` / `(vel …)`, and `vel*`, `vel+`, `note+`,
`(gate …)`, `(plock NAME …)`, and `on`'s `(note …)` / `(vel …)`. A clocked
`plock` value lowers to `(:defer :plock raw key label)`, its label folded into
the counter key, so two `plock`s on one row step their own sequences. Anywhere else (retiming,
figure transforms, selector arguments, route destinations) a `seq` reads
per cycle whatever its clock, since those values shape the whole cycle
before any hit plays.

**Counting.**
- `:hit` counts the hits the value applies to, **across cycles**: four notes
  over a seven-hit cycle drift against the rhythm and repeat every 28 hits.
  Inside `(on SEL …)` it counts only the matched hits, so
  `(on left (note+ (seq :hit 0 7 12)))` is a left-hand melody.
- `:fig` counts figure occurrences among those same hits: it moves on when a
  hit belongs to a different figure (or cycle) than the previous one.
- `:span` is stateless: a hit at offset `o` of an `L`-unit cycle takes member
  `floor(o / L · n)`.
- `:cycle` is the cycle index, as before.

Counters advance as hits are emitted, in time order, and restart when the
transport jumps (the same rule as hand/velocity threading, §6.3).

**Nesting.** A member may itself be a list. A nested `seq` whose clock is
`:hit` or `:fig` steps once per **visit** of its parent — `(seq :hit 0 (seq
:hit 7 12))` plays 0 7 0 12 0 7 … — while a nested `:cycle` list (or plain
`(7 12)`) moves per cycle, as in Tidal: `(seq :hit 0 (7 12))` plays 0 7 0 7 …
on cycle 0 and 0 12 0 12 … on cycle 1. A nested `:span` reads the hit's
position.

**Evaluation.** A per-hit word whose argument needs a hit clock is not
applied during evaluation (which is memoized per cycle); it rides on the
event as a deferred op `(kind raw key)` — `key` unique per route word — and
`emit` resolves it against that key's counters, after the words applied
during evaluation.

## 7.3 Lit items (kind panel)

While the transport plays, the jaki kind panel rings each row item that is
being applied to the hit sounding on that row: an `(on SEL …)` whose selector
(narrowed by any `every` inside it) matches the hit, and an `(every n …)` /
`(every-fig n …)` whose gate is open for the hit's cycle / figure.

- Route words are stepped with their index (`:widx`, the row item index).
  Those items add a post op `(:tag sel idx)`; it pushes `idx` onto `:lits`
  of every event `sel` matches (`(every n)` → `(:every n)`, `every-fig` →
  `(:fig-every n)`). A structural word an `on` ignores adds no tag.
- A route with tags emits, per hit, `(gen-mark mask route at)` and
  `(gen-mark 0 route (+ at dur))`: the bitmask of its items at the hit's
  onset, cleared at its end. `gen-mark` stamps the boundary's audio sample
  plus `at` beats; a mark earlier than the newest drops the newer ones, so the
  next hit's onset replaces a pending clear.
- The host publishes the latest sounded mark as
  `SEQ.generator-mark-<id>-<route>`; the row's sexp-slot binds it as `:lit`
  (a render binding: a hit repaints the slot, never re-runs the view).

## 7.5 Rules: `(rule TRIGGER STAGE… ACTION…)`

Full design: docs/jaki-row-processes-spec.md (epic eseq-1sr5; `proc` is the
old name and still parses). In short, a route word that runs a small chain inside the evaluator's figure and
hit fold, in Lisp:

- **TRIGGER** is any `on` selector (§7.1.1). The chain *steps* on every hit it
  holds for.
- **STAGEs** fold one number `x` from 0, with keyword args: sources
  `(acc :by :min :max)`, `(coin :p)`, `(rand :min :max)`, `(count :n)`,
  `(cyc v…)`, `(chan "name" d)`, a bare number; transforms `sin`, `abs`,
  `(pow :e)`, `(remap :in-lo :in-hi :out-lo :out-hi)`, `(clamp :min :max)`,
  `(cmp :op >= :to)`, `(quant :step)`. Any number arg may be a nested source:
  `(acc :by (coin :p 0.5) :min 0 :max 16)`.
- **ACTIONs**: held `(note+ $1) (vel* $1) gate* dashdecay* dotdecay* basevel*`
  (`$1` is the stages' value, `x` the older spelling; sample and hold); event words with a number,
  `(note+ -12)`, `rest`, …, act on the stepping hit when `x >= 0.5`
  (`(then W…)` groups several); `(next W…)` on the next figure, including
  `(fast n)`; note ops
  `(scale :minor :root C)`, `(harmony :track n :amount a)`, `(snap "chan")`
  on every hit the chain steps on.
- `(if COND w… (else w…))`: COND over numbers and `$` context (`$1` the
  stages' value, `$n` the rule's steps so far, `$vel`, `$cycle`, `$fig`,
  `$rep`; `= != < > <= >=`, `and or not`, `+ - * / mod min max abs`). Every
  word after COND runs when it holds; event words act on this hit, figure
  words (`half rev swap ghost (fast n)` and `(minvel v)`-style velocity
  words) on the next figure. Bare figure words straight in a rule also mean
  the next figure. A rule with no stages always acts.
- Randomness is a pure hash of (seed, chain, node, cycle, figure, hit).
  `(seed n)` sets the route's seed; the default is the route index.
- Rev 1 spellings still parse: positional args `(acc 0.07 0 1)`, `(+ note)`,
  `(* dashdecay)`, `(scale -1 1 0 1)` as remap.

`(scale …)`, `(harmony …)` and `(snap …)` are also plain route words (and
`on` words): they move every hit's final note at emit (§13 of the row
processes spec).

```lisp
(jak "yo" :16
  (fig (. . -)) (fig (. -))
  -> 0 (rule any (acc :by 1 :min 0 :max 8) (note+ $1)) (scale :minor :root C)
  -> 1 (rule left (coin :p 0.5) (next rest))
  -> 2 (rule (and dash head) (acc :by 0.07) sin
             (remap :in-lo -1 :in-hi 1 :out-lo 0 :out-hi 1) dashdecay*))
```

## 8. Runtime Model

### 8.1 Generator integration

One `def-sequencer` per Jaki instance:

```lisp
(import alez.jaki.core)

(def-sequencer "jaki-kit"
  :resolution :16
  :tick
  (do
    (alez.jaki.core/init :16)
    (let ((base (alez.jaki.core/pat . . -
                  (every 2 swap)
                  (* (cyc 1 2)))))
      (do
        (alez.jaki.core/emit base 0)
        (alez.jaki.core/emit (alez.jaki.core/shift base 1) 1)
        (alez.jaki.core/emit
          (alez.jaki.core/filter base '(:hand :left)) 2)))))
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

## 11.1 Implementation notes (phases 1–3, bead eseq-5k5)

Phases 1–3 first shipped as a pre-package module and moved in phase 4 to
`content/packages/alez.jaki/src/core.lisp` (`(module alez.jaki.core)`). Tick
bodies are authored on the UI VM but run on the scheduler VM via
`def-sequencer`'s quoted-source shipping — the library must never be captured
as UI-VM closures. Low-level callers import it as
`(import alez.jaki.core :as jaki)`; tick source should use fully qualified
`alez.jaki.core/…` names because aliases do not travel in the serialized tick
body. Deviations from the surface sketched above, all forced by measured
eseqlisp semantics:

- **`jaki/pat` is variadic macro sugar over `jaki/from-list`.** eseqlisp
  `defmacro` supports `&rest` parameters and `,@` splicing, so the authored
  spelling is `(jaki/pat . . - (every 2 swap))`. Code that constructs pattern
  forms dynamically can call `(jaki/from-list forms)` directly.
- **Quotes inside `:tick` now survive.** def-sequencer's captured tick data is
  re-serialized with `format_lisp_source` for the scheduler VM, which used to
  erase `'`. The compiler now captures inner quotes as `(quote x)` data (tick/
  init capture only) and the formatter prints that pair back as `'x`.
- **Transform arguments are data, not functions.** `(jaki/every base 4 'rev)`,
  `(jaki/for-hand base :left '(stac))`, `(jaki/filter base '(:hand :left
  :symbol :dash))` — a function value can't be mapped back to a transform
  name, and filter axes ride in one quoted plist because Lisp-defined
  functions are fixed-arity. `jaki/xform` appends any quoted transform.
- **`jaki/emit` is `(jaki/emit pat track)`**, with `(jaki/emit* pat track
  (dict :vel-scale … :note …))` for options. `:quantize`/`:chord`/`:pan`/
  `:speed` passthrough needs conditional seq-emit call shapes — deferred to
  the package bead. `(jaki/init :16)` is called at the top of `:tick`
  (idempotent) because `def-sequencer`'s `:init` is parsed but not yet run.
- **`(align n)` pads duration silently; `(align n :pad)` generates dot padding
  events** with hand/velocity threading (Swift `@N` behavior); the `@N-` dash
  tail was dropped from the surface.
- **`(swap)` flips the emitted hand assignment for the cycle** but does not
  disturb the threaded alternation (the ending hand ignores it).
- Cycle lengths are provably integral (fast expansion multiplies raw units by
  m, fit/slow/align are integer-valued), so each resolution tick belongs to
  exactly one cycle; offsets within a cycle are exact `(num den)` rationals
  and window membership is integer comparison, as §8.3 requires.
- The per-cycle memo is a capped assoc list in scheduler-VM globals keyed by
  `(pattern-id cycle hand vel-state…)` — dict keys can't be synthesized at
  runtime in eseqlisp. Pattern ids derive from `(source body)` plus transform
  tags, so hot-reloading a pattern re-keys naturally.
- Velocity/hand threading crosses cycles through f64 generator state cells
  (`jaki-cycle`/`jaki-hand`/`jaki-vel`/`jaki-pwd`/`jaki-streak`); contiguous
  cycle advances roll the ending state forward, transport jumps restart from
  defaults while the closed-form cycle index (§8.1) stays exact.

## 11.2 Tier-2 route surface (`jak` macro)

The def-sequencer skeleton is now machine-written. The package module
`alez.jaki.surface` exports the `jak` macro; callers opt in explicitly:

```lisp
(import alez.jaki.surface :refer (jak))
```

`alez.jaki.core/run` interprets the body:

```lisp
(jak "kit" :16
  . . - . (every 2 swap)
  -> 0
  -> 1 left
  -> 2 (shift 1) stac
  -> 3 accent (vel 0.7))
```

- The body is split at top-level `->` symbols (`->` is an ordinary eseqlisp
  symbol). Segment zero is the pattern — the exact `jaki/pat` grammar — and
  each later segment is `track word…`. No routes ⇒ the pattern plays on
  track 0.
- Route words map onto the existing pattern functions: `left`/`right`
  (hand filter), `accent` (accent filter), `rev`/`stac`/`ghost`/`swap`,
  `(shift n)`, `(rot n)`, `(trunc n)`, `(every n t)`, `(for-hand h t)`,
  `(fast n)` / `(slow n)` (retime just this route; `n` may be `(cyc …)` for
  conditional retiming — threading state is keyed per pattern id so routes
  with different cycle structures don't fight over the shared cells),
  and emission options `(vel s)` (`:vel-scale`) and `(note n)`. Unknown
  words are ignored.
- `(on SEL word…)` scopes words to the events or figures a selector matches
  (§7.1); `(vel* s)`, `(vel+ d)` and `(note+ n)` are per-event velocity and
  transpose adjustments usable at the top level or inside `on`, and
  `(plock NAME v)` a per-hit parameter lock (§7.1.2).
- Route-word `stac` is a **post op** (gate cap at 1/4 unit), not the
  figure-level xf flag, so it composes with the gate-extending filters in
  authored word order: `left stac` filters to the left hand and then caps
  the extended legato gates; `stac left` caps first and the filter's §7
  legato extension wins. Figure-segment `(stac)` inside the pattern keeps
  the scale-aware xf behavior from §4.6.
- `(every n w)` dispatches on the wrapped word: post-op words (`stac`,
  `(shift k)`, `left`/`right`/`accent`, nested `every` of those) lower to a
  cycle-gated post op applied in authored word order; xf-able words keep the
  §4.6 `every` transform path.
- `(quant tb)` — post op that snaps every surviving event offset to the
  nearest multiple of the timebase grid (straight or triplet: `:16`, `:8t`,
  …), wrapping into the cycle like `shift`. Composes in word order:
  `left (quant :16)` quantizes the left hand's events. The grid resolves to
  exact rational units at route time ((beats tb)/unit over denominator 96),
  so the memoized evaluator stays resolution-independent and tuplet
  membership needs no epsilon. Distinct from the stream-layer quantizers
  (MIDI FX quantize, `seq-emit :quantize`), which are forward-push-only
  deferrals — the post op re-places events musically and may pull them
  earlier.
- The route destination and the emission options are per-cycle arguments:
  `-> (cyc 1 3) left` bounces the route between tracks each pattern cycle,
  and `(note (cyc 0 4 12))` / `(vel (cyc 1 0.5))` cycle the transpose /
  velocity scale. Resolution uses the routed pattern's own cycle index
  (post-retime), via `resolve-arg` in `emit*`.
- Multi-voice: when the first body element is a list containing a top-level
  `->`, every element is one voice line with its own pattern and routes:
  `(jak "kit" :16 (. . - . -> 0) (- . . . -> 1 stac))`.
- The macro expands to the plain skeleton `(def-sequencer name :resolution
  res :tick (do (alez.jaki.core/init res) (alez.jaki.core/run '(body…))))`,
  so the VM split is unchanged: the body ships as quoted source and is
  interpreted scheduler-side each tick.
- Jaki is a validated factory package (`alez/jaki`, entry
  `alez.jaki.surface`). Both authoring and scheduler scratch runtimes receive
  the scoped package module path. There is no source sniff, textual prepend,
  embedded fallback, or unconditional UI-VM load.

## 11.3 Phase 4 package layout

```text
content/packages/alez.jaki/
  manifest.json
  src/core.lisp       ;; alez.jaki.core
  src/surface.lisp    ;; alez.jaki.surface; exports jak
```

The factory package rung follows user-installed packages and precedes ordinary
factory modules. This preserves user-package shadowing while making the
checked-in package available in both development and bundled `Resources/`
layouts. Import is deliberately required; bare `jak` without `:refer (jak)` is
not supported.

Phases 5 and 6 remain blocked: the `def-sequencer` param/telemetry/UI contract
in `lisp-sequencer-remaining.md` has not landed, and generator pitch seeding
does not yet exist. They must use those shared contracts rather than adding
Jaki-only host plumbing.

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
