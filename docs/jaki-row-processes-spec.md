# Jaki row processes: per-row process chains, written in Lisp

Status: spec rev 1, 2026-09-29. Epic: `bd list --label jaki-procs`.

## 1. Goal

A neuron in `alez.neural.variable-reset` owns a process patch: cards such as
acc, rand, cmp, harmony and transpose, chained on each fire
(docs/graph-node-processes-spec.md). This spec gives a **jaki row** the same
thing: a list of process chains that run on the row's hits and can reshape
what it plays.

```lisp
(jak "yo" :16
  . . - . -
  -> 0 left
       (proc left (coin 0.5) (next rest))            ; heads/tails on each left hit
  -> 1 right
       (proc (and dash head) (acc 0.07) sin    ; evolving dash decay
             (scale -1 1 0 1)
             (* dashdecay)))
```

**Constraint: all of it is Lisp.** It lives in `content/packages/alez.jaki`.
This epic adds no Rust natives and changes no scheduler code. Rust is allowed
only in tests that drive the Lisp. The feature is also meant as evidence that
the scripting layer can carry a feature of this size.

That rules out reusing the track/neuron process runtime (`def-process`,
`scheduler/node_process.rs`). That runtime is Rust-hosted, and its `read` /
`rand` natives error outside a process eval context, just as `state-get` /
`gen-rand` error outside a generator tick. Jaki row processes are their own
small process language inside the jaki evaluator. They borrow the neuron
editor's look and the process vocabulary (acc, rand, cmp, transpose), not its
runtime.

## 2. Why inside the evaluator (and not on emitted hits)

`alez.jaki.core` evaluates a **whole cycle in one pass**. `eval-figs` folds
figure after figure. `fold-events` folds hit after hit inside a figure, and
threads the hand and the velocity state (`st` = `:cur :pwd :streak`) through
both. `store-state` / `roll-state` carry that state across cycles in
per-pattern generator state cells (`cell p "jaki-vel"` …).

A process that runs inside that fold therefore:

- sees every hit in order, already tagged with `:hand :sym :hit :accent :fig`;
- can hold memory (an accumulator) the same way the velocity model does;
- can leave a decision that the **next figure** reads before it is evaluated,
  which is how "flip a coin, rest the next figure" works (§6);
- can modulate the velocity model itself: `(* dashdecay)` scales the
  `dash-decay` param that `next-vel` uses, rather than patching velocities
  after the fact.

A chain run on already-emitted hits could do none of the structural things.

## 3. Surface

A row process is a **route word**, so it works both in a typed `(jak …)`
form and in the kind's document:

```
(proc TRIGGER STAGE… ACTION…)
```

- **TRIGGER** (required): any `on` selector (core.lisp `norm-sel`): `any`,
  `left`, `right`, `dot`, `dash`, `head` (hit 1), `tail` (hit 2), `accent`,
  `(fig n)`, `(rep n)`, `(every n)`, `(not s)`, `(and s…)`, `(or s…)`. The
  chain **steps** on every hit the trigger holds for.
- **STAGEs** (zero or more): a pipeline over one number, `x`, which starts
  at 0 on each step. Sources replace `x`; transforms map it (§4).
- **ACTIONs** (one or more, always last): what `x` does (§5).

A row may carry any number of `proc` words. Chains are indexed in authored
order (chain `k` of that route). Each chain has its own state; chains do not
see each other's `x` (v1).

`proc` words are collected by `route-step-word` into the route pattern's
`:procs` list. The route's `:id` gets the proc's source text appended, as
every other route word does, so memos stay keyed correctly.

## 4. Stages

| Stage | Kind | Value |
|---|---|---|
| `(coin p)` | source | 1 with probability p, else 0 (hashed, §7) |
| `(rand)` / `(rand lo hi)` | source | uniform in [0,1) / [lo,hi) (hashed) |
| `(acc step)` / `(acc step lo hi)` | source | previous acc + step, wrapped into [lo,hi); default [0,1). Stored per chain |
| `(count n)` | source | 0,1,…,n-1,0,… per step. Stored per chain |
| `(cyc v…)` | source | per cycle, same rule as route args |
| `(chan "name" d)` | source | `chan-get` (per-cycle snapshot, one-chunk latency) |
| `sin` | transform | sin(2πx) |
| `(scale a b c d)` | transform | linear map [a,b] → [c,d], unclamped |
| `(clamp lo hi)` | transform | clamp |
| `(cmp OP t)` | transform | 1 or 0; OP is `>` `<` `>=` `<=` `=` |
| `(quant s)` | transform | round to a multiple of s |
| `abs`, `(pow e)` | transform | as named |

`acc` and `count` advance only on steps (trigger hits), never per tick.
`sin` and `(pow e)` use the eseqlisp `sin` / `floor` builtins; everything else
is arithmetic.

## 5. Actions

Actions run in authored order after the last stage.

### 5.1 Held modulation (continuous)

| Action | Effect |
|---|---|
| `(* dashdecay)` | the dash-decay param × x, clamped to [0,1] |
| `(* dotdecay)` | the dot-decay param × x, clamped to [0,1] |
| `(* basevel)` | the base velocity × x, clamped to [0,1] |
| `(* vel)` | the hit's velocity × x, clamped to [0,1] |
| `(+ note)` | + round(x) semitones on the hit (`:nadd`) |
| `(* gate)` | the hit's gate × max(0,x) |

These are **sample and hold**. A step stores `x` as the chain's held value,
and every hit on the row from then on (triggering or not) uses the held value,
until the chain steps again. Before a chain's first step the held value is
the identity: 1 for `*`, 0 for `+`.

The decay/base targets multiply the param **as the figure resolved it**
(defaults, then the row's `(dashdecay v)` etc. words, then `vel-overrides`).
So `(* dashdecay)` means "whatever the slot says, times x", which is the
behaviour asked for. They are applied per hit in `mk-hit` / `next-vel`, so
they are exact: nothing downstream has to be reconstructed.

Worked example: `(proc (and dash head) (acc 0.07) sin (scale -1 1 0 1)
(* dashdecay))` steps once per dash, on its first hit. The held value is
already updated when the dash's second hit (the only hit dash-decay touches)
reads it. The multiplier traces one sine period about every 14 dashes
(1 / 0.07): from half, up to full, back through half to silent, and home.

### 5.2 Gated actions (on this hit / on the next figure)

`x ≥ 0.5` counts as **true**; `(coin p)` gives exactly 1 or 0.

| Action | Effect when x is true |
|---|---|
| `(then W…)` | apply event words W to **this** hit |
| `(next W…)` | apply figure words W to the **next figure** (§6) |

Event words allowed in `then`: `rest` (drop the hit), `stac`, `(vel* v)`,
`(vel+ v)`, `(note+ n)`, `(note n)`, `(gate s)`. A dropped hit still
advances hands and velocity state, like a filtered hit: the rhythm's
threading does not move because of a coin.

## 6. Next-figure actions

`(next W…)` sets a per-chain **pending** flag. At the start of each
`eval-fig`, every pending chain's words are applied to that figure and the
flags are cleared. The words themselves are fixed in the chain, so pending is
one number per chain, which fits scalar state cells (§8).

- The "next figure" after a cycle's last figure is the first figure of the
  next cycle. The flag rides the cycle-end state like the hand does.
- If several hits in one figure trigger, the flag is simply set (not
  counted).

**Length-preserving words only.** `fig-len` and the super-cycle tables
(`len-memo`, `lens-memo`, `locate`) compute cycle lengths without running
processes. A process must therefore never change a figure's length. Allowed
in `next`:

| Word | Meaning |
|---|---|
| `rest` | the figure evaluates normally (hands, velocity) but its hits are dropped |
| `rev`, `swap`, `ghost`, `stac` | the existing figure transforms |
| `(note+ n)`, `(vel* v)`, `(vel+ v)`, `(gate s)` | applied to every hit of the figure |
| `half` | **fit** halftime: `half` then `:fit` back to the figure's own unit count, so the figure keeps its length (§6.1) |

Any other word inside `next` (`fast`, `slow`, `trunc`, `(rot n)` on a
figure with padding, `(* n)`) is rejected when the route is prepared.
Because this is a live-coding surface, "rejected" means ignored, with a
status message; the rest of the pattern keeps playing.

### 6.1 Halftime

- **Fit halftime (in scope).** `(next half)` is `half-events` followed by a
  `:fit` time-mod to the untransformed unit count. The figure plays its
  halftime shape squeezed into its original span, and cycle length is
  untouched. This reuses `half-walk` and the `:fit` kind that `eval-fig`
  already handles.
- **True halftime (out of scope, bead .7).** Making the next figure *longer*
  makes cycle length depend on coin outcomes. `locate` finds the cycle from
  the tick in closed form, assuming lengths repeat with period P. Supporting
  data-dependent lengths means keeping a running cycle offset in state and
  giving up exact seek (a jump would have to replay or restart). That is a
  design of its own.

## 7. Randomness: hashed, not streamed

`gen-rand` is a stream: each call advances generator state. Jaki results are
memoized per cycle (`eval-cycle`), and `roll-state` re-evaluates skipped
cycles. A stream would give different coins depending on how the evaluator
reached a cycle.

Row processes use a **pure hash** instead:

```
u = hash(seed, chain k, stage s, cycle, figure index, hit index) ∈ [0,1)
```

This is implemented in Lisp, for example as `fract(sin(dot(key, primes)) ·
43758.5453)` with a guard against the low-bit patterns of the sine hash (the
unit tests check the distribution, §10). Consequences:

- The memo stays valid, and seeking to a cycle reproduces its coins exactly.
- The same position always makes the same choice. Replaying bar 12 replays
  bar 12's coins, which is usually what is wanted live.
- `(seed n)` as a route word, or `:seed n` in the kind document, sets the
  row's seed. Changing it rerolls every chain on the row. The default seed is
  the route index.

## 8. State

`state-set!` stores **numbers only**, so chain state is flattened into
scalar cells keyed by the route pattern's cell prefix, next to `jaki-vel`:

| Cell | Meaning |
|---|---|
| `jp<k>:acc` | accumulator |
| `jp<k>:n` | count |
| `jp<k>:hold` | held value (§5.1) |
| `jp<k>:pend` | pending next-figure flag (§6) |

The chain state travels **inside `st`** as a `:procs` list of per-chain
dicts. Everything that already threads `st` therefore threads it for free:
`fold-events`, `eval-figs`, `roll-state`, `ensure-state`. `load-state` /
`store-state` read and write the four cells per chain.

- **Memo key.** `eval-cycle` adds the flattened proc state to its key (next to
  `:cur :pwd :streak`).
- **Period.** A route with any `proc` gets `:period 0`: hashes read the
  cycle, and accumulators make state non-periodic. The memo falls back to
  exact cycle keys. Within a cycle every tick still hits the memo, because
  state is only stored on cycle advance.
- **Jumps.** On a transport jump, `ensure-state` resets threading to the
  defaults, and chain state resets too (acc = lo, count = 0, hold =
  identity, pend = 0). This matches the existing hand/velocity rule. Coins do
  not depend on state, so they stay exact across jumps.

## 9. The kind (GUI)

**Document.** A row gains a `:procs` field: a list of chain data, each a
plain literal like the row's `:mods` (scene slots hold literals). For
example `("left" ("coin" 0.5) ("next" "rest"))`, with the same string/number
encoding as sexp-slot items. `alez.jaki.doc/row-segment` appends one
`(proc …)` word per chain after the mods, through the existing
`route-datum`. Undo and scenes come from the same scene-slot write as `:mods`.

**Editor.** Each row gets a `procs` affordance (a count badge; click to
expand). This opens the expanded row editor, styled like the neuron editor
(`gvr-expanded-editor`), with one line per chain:

```
[trigger ▾ left]  [coin 0.5]  [next ▾ rest]                       ×
[trigger ▾ dash head]  [acc 0.07]  [sin]  [scale -1 1 0 1]  [* dashdecay]   ×
+ chain
```

- Stage and action cards are dropdown heads with number fields for their
  arguments; `+` appends a stage.
- It is linear by design: a chain is a pipeline, so no cables are needed.
  This also keeps the editor clear of the Rust-backed lane patch bay.
- The hit strip preview (`alez.jaki.core/preview`) already threads `st`, so
  it shows process effects. It starts from default state, so a long-running
  accumulator can differ from what is playing; the strip is a preview, not a
  meter.

## 10. Testing

Most of it is testable **without a tick**. `preview` / `eval-at` are pure
given `st`, and the hash needs no natives. Tests go in the existing jaki
scratch-VM harness (`crates/sequencer/src/lisp_host/tests.rs`, alongside the
`jaki_*` tests):

- The hash: determinism, and a rough uniformity check over 10k keys (bucket
  counts within tolerance).
- Coin/next: `(proc left (coin 1) (next rest))` silences every figure after
  the first; with `(coin 0)` nothing changes. Deterministic mixes at p = 0.5
  match a recorded expectation.
- Dashdecay: with `(proc (and dash head) (acc 0.25) (* dashdecay))`, the
  second hit of each dash equals `clamp(first × dashdecay × held)`, and
  successive dashes see the stepped acc.
- Length invariance: for every allowed `next` word, `fig-len` equals the
  evaluated `:dur`, including fit halftime.
- Threading: a cycle-end pending flag applies to the next cycle's first
  figure; `roll-state` over skipped cycles gives the same state as ticking
  through them.
- One tick-driven test that emits through `seq-emit` with a process, to prove
  the state cells round-trip.
- For the kind: a capture render of a row with two chains (render-panel
  skill).

## 11. Out of scope / later

- **Harmony (bead .6).** "Set harmony from track 4 / slot 3" needs a harmony
  read in generator context, and none exists: process `read` natives are
  walled off. The Lisp-only route is a track process that publishes the
  chord as a 12-bit pitch-class mask into a channel, plus a jaki stage
  `(harmony "chan")` / action `(snap note)` that reads it with `chan-get`.
  This needs a check that channel values carry integers that large exactly
  (they are f64, so 4095 is safe), and a decision on where the publishing
  process lives.
- **Row-to-row reads.** One chain reading another row's hits or held value
  can live in state cells, but it couples route evaluation order; deferred.
- **Chains reading each other's x.** A shared per-row bus; deferred until
  someone wants it.
- **True halftime** (§6.1, bead .7).
