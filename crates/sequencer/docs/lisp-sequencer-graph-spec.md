# Graph-Mode `def-sequencer` — Implementation Spec

> Status: design complete, not yet implemented. This is a hand-off spec.
> It builds on the **working** tick-mode `def-sequencer` (see `lisp-sequencer-spec.md`
> and `lisp-sequencer-remaining.md`). Tick mode ships today; graph mode is additive
> and must not regress it.

## 0. One-paragraph summary

`def-sequencer` gains a second shape, **graph mode**, for authoring node-graph
sequencers (neural-class) in lisp. A sequencer declares a **shape** (the node field),
one or more **`def-node`** prototypes (behavior), and **`edges`** (connectivity).
Each tick the engine runs a native **gather/scatter** loop over the graph: it gathers
weighted, delayed contributions along edges into each node, the node's lisp `:update`
decides whether to fire, and firings scatter back along out-edges after a per-node
delay. The end goal — the acceptance test — is that the existing hardcoded neural
sequencer (`neural.rs`) can be **reproduced as a `def-sequencer`**, including
per-neuron delay tuning, transpose-through-feedback, dampening, seeding from the
step sequencer, and `max-poly` voice-stealing.

## 1. Why / design principles (carried from the design sessions)

These are non-negotiable; every decision below follows from them:

1. **Engine owns the hot loop and all timing math.** Lisp expresses *intent* (a
   per-node update rule, a gather formula), never sample arithmetic, never the
   quantize/lookahead/tempo conversion. The cost model: O(edges) native gather +
   O(nodes) lisp update per tick.
2. **Three tiers of node field, and the syntax makes the tier obvious:**
   - **Intrinsic fields** (top-level `def-node` keywords): every node has them, the
     **engine reads** them to schedule/route. `resolution`, `delay`, `quantize`,
     `route`, `seed-from`, `reduce`. The author sets defaults; they're never declared
     into existence.
   - **Behavioral params** (`:params` block): knobs *this author invented*, read by
     the lisp `:update`/`:gather`. The engine only stores them. `threshold`,
     `transpose`, …
   - **State** (`:state` block): runtime, per-instance, optionally leaky. `energy`.
   - Litmus test: *"must the engine understand this field to do its job?"* Yes →
     intrinsic. No → `:params`.
3. **gather lives on the edge, reduce lives on the node.** The synapse decides what
   current it injects (gather); the dendrite/soma decides how currents combine
   (reduce). A node can receive from several edge-sets at once; the node's `reduce`
   folds them all.
4. **Per-instance temporal identity is the compositional surface.** `resolution` /
   `delay` / `quantize` are per *node instance*, edited in the UI, serialized per
   pattern. This is how the user tunes groove ("node 1 delay 3; node 2 delay 5,
   quantize 4t"). It is **v1, not deferred** — neural is not faithfully reproduced
   without it.
5. **One sequencer = one serializable unit.** `def-node` and `edges` are *grammar
   inside* `def-sequencer`, not standalone callables; they have no meaning outside it.

## 2. Surface syntax

### 2.1 `def-sequencer` (graph mode)

```lisp
(def-sequencer NAME
  :shape  (grid 8 8)            ; the node field — see §2.4. REQUIRED in graph mode.

  ;; sequencer-level engine config (§5)
  :energy-decay        0.9
  :reset-every         (bars 4)
  :seed-on-reset       0.0
  :max-poly            4
  :max-poly-selection  :strongest

  (def-node ...)               ; one or more node prototypes (§2.2)
  (edges ...))                 ; zero or more edge sets (§2.3)
```

- Graph mode is selected by the presence of `def-node` sub-forms. `:tick` (no
  `def-node`) selects the existing tick mode. The two are mutually exclusive shapes
  of the same form.
- `:shape` is **sequencer-level**, not per-node (the grid *is* the sequencer's
  identity). v1: exactly one `def-node`, applying to every cell of the shape.
  Multi-prototype (assigning prototypes to regions of the shape) is a later
  extension and is explicitly out of scope for v1.

### 2.2 `def-node` (a node prototype)

```lisp
(def-node NAME
  ;; ── intrinsic temporal identity (engine-read; PROTOTYPE DEFAULTS, per-instance editable) ──
  :resolution :16            ; eval/fire grid (Timebase)
  :delay      0              ; delay_steps; integer steps of resolution; engine floors to 1
  :quantize   :off           ; optional fire-time snap to a (possibly different) grid

  ;; ── intrinsic I/O routing (engine-read; per-instance editable) ──
  :route      none           ; OUTPUT track index (none = internal-only relay)
  :seed-from  :route         ; INPUT track for seeding; default = follow :route (§4)

  ;; ── intrinsic input fold ──
  :reduce     sum            ; sum | max | min | product | count (default sum)

  ;; ── behavioral params (author-invented; lisp-read) ──
  :params ((threshold       :float 0 4 :default 1.0)
           (transpose       :int  -24 24 :default 0)
           (dampen-amount   :float 0 1 :default 0.0)
           (dampen-recovery :float 0 1 :default 0.95))

  ;; ── runtime state (per-instance; :leak applies elapsed-time decay) ──
  :state ((energy :leak (per-step :energy-decay)))

  ;; ── the only custom logic: the per-node update rule ──
  :update <expr>)
```

Accessors available inside `:update` (and where noted, `:gather`):
- `(node-state self KEY)` / `(node-set! self KEY V)` — runtime state cells.
- `(node-param self KEY)` — behavioral params (with per-instance plocks applied).
- `(node-route self)` / `(node-quantize self)` — **intrinsic** accessors (kept
  distinct from `node-param` so the two namespaces don't leak).
- `(node-input self)` — the reduced gather result for this node this tick.
- `(node-input-event self)` — the **payload** that arrived (Ext 1, §3.1).
- `(node-index self)` — instance index within the shape.
- `(routed? self)` — true if `:route` is set.
- `(entity-emit self :track … :note … :relay … :quantize …)` — emit an event (§3.1).
- `(dampen-incoming self AMOUNT)` / `(recover-incoming self FACTOR)` — Ext 2 (§3.2).

### 2.3 `edges` (an edge set)

```lisp
(edges :from neuron :to neuron     ; source prototype -> target prototype (may differ)
       :topology (all-to-all)       ; §2.5 — materialized ONCE at build time
       :gather (max 0 (- (edge :weight) (edge :dampening)))  ; per-edge contribution
       :params ((weight    :float -1 1 :default 0.0)
                (dampening :float  0 1 :default 0.0)))         ; edge fields (incl. runtime state)
```

- `:gather` is an expression over `(edge KEY)` (this edge's params/state) and
  `(node-state src KEY)` (the source node's state). It returns the scalar
  contribution this edge injects into its target this tick.
  - Reading source state → analog/continuous propagation.
  - Reading only edge fields (as above) → spike propagation, magnitude independent
    of source activation. Neural is this case.
- `:gather` compiles to a **native kernel** (no per-edge lisp). An arbitrary lisp
  lambda here is an opt-in slow path and is **out of scope for v1**.
- Edge `:params` double as edge **runtime state** (e.g. `dampening` is mutated at
  runtime by `dampen-incoming`). Author-set defaults serialize; runtime values do not.

### 2.4 Shape generators (`:shape`)

Materialize the node set + its addressing + default UI layout. v1 set:
- `(grid R C)` — `R*C` nodes addressed by `(r,c)`, flat index `r*C+c`.
- `(line N)` — `N` nodes, index `0..N`.
- `(ring N)` — like line, with wrap semantics for neighbor topologies.

### 2.5 Topology generators (`:topology`)

Declare *which* edges exist (the adjacency), evaluated **once at build time** into a
sparse edge set. Distinct from edge *values* (the `:params`/plocks). v1 set:
- `(all-to-all)` — every source to every target (weight 0 = effectively no edge).
  **This is the only v1-required topology** (reproduces neural's dense weight matrix).
Later: `(grid-neighbors)`, `(ring)`, `(random :density D)`, `(edges-fn (lambda (from to) …))`,
modular/cluster topologies. Out of scope for v1.

## 3. The three extensions beyond the generic model

Recreating neural revealed exactly three things the generic gather/reduce/node model
does not yet express. They are independently shippable (see §7 ordering).

### 3.1 Ext 1 — payload-carrying signals (REQUIRED for neural character)

The signal flowing along an edge is `(magnitude, payload)`, not just a scalar. The
payload is the **seed event** (note / sample / velocity) that originated from a step
trigger. A firing node re-emits its incoming payload, transposed:

```lisp
(entity-emit self
  :track    (node-route self)
  :relay    (node-input-event self)                 ; carry the event that triggered me
  :note     (+ (event-note (node-input-event self)) ; transpose accumulates around feedback loops
               (node-param self :transpose)))
```

Without this you can only emit synthesized-from-scratch notes; *with* it, a seed
ripples through the net and is re-pitched on every hop — the running sum of
transposes around a feedback cycle is the entire Aphex-Twin melodic-cascade behavior.
`event-note` / `event-sample` / `event-vel` read fields off a relayed event.

Native reference: `firing_candidate` (`neural.rs:827`) clones `source_events[idx]`,
adds the neuron's `transpose`, and re-stamps `EventSource::Network`. `source_events`
is set during propagation (`apply_due_propagations`, `neural.rs:818`).

### 3.2 Ext 2 — edge-state writes from a node's update

A node's `:update` can mutate its **incoming edges'** state. Neural's *dampening* is
per-edge synaptic depression controlled by the *target* node's params:

- On fire: `dampen-incoming self amount` → for each incoming edge `e`, increase
  `e.dampening` (capped 1.0) for the edges that actually triggered this node.
- On non-fire: `recover-incoming self factor` → `e.dampening *= factor`.

This extends the update's write-scope from "own state" to "incoming edges." Native
reference: `commit_firing` (`neural.rs:886-893`) and `recover_non_firing_neuron`
(`neural.rs:901`); note it only dampens edges whose `incoming_triggers` fired
(`neural.rs:815`), so the engine must track which edges contributed to a firing this
tick. `dampening` then subtracts from `weight` in gather (`neural.rs:809`).

### 3.3 Ext 3 — sequencer-level voice-steal + reset/seed (engine config)

Not per-node lisp; engine config applied around the node loop:
- `:max-poly` + `:max-poly-selection` — cap simultaneous firings, steal by a rule
  (`:strongest`, etc.). Applied *after* all firing candidates are gathered and sorted.
  Native reference: `select_firing_candidates` + `candidates.sort_by_key((fire_sample,
  neuron_idx))` (`neural.rs:699-700`). Generators already inherit `max_poly` from the
  Phase-2 work — reuse that path.
- `:reset-every` (bars) + `:seed-on-reset` (per-node initial energy) — periodic reset
  + re-seed. Native reference: `reset_state` / `next_reset_beat` (`neural.rs:583`,
  `633-650`), `seed_on_reset` (`neural.rs:578`).

## 4. Seeding (input from the step sequencer)

The built-in step sequencer is the **input device**; def-sequencers are transformers
that subscribe to tracks, mangle, and re-emit.

- **`seed-from`** is the per-node INPUT track — the dual of `route` (OUTPUT). It
  defaults to `:route` (i.e. "the track I write to is the track I listen to"),
  preserving today's zero-config behavior, but can be set explicitly to **decouple
  input from output** (seed from track 0, play out to track 2 — the user's "extend a
  mini-beat" workflow).
- **Mechanically, seeding = inject a fire into the seed node.** It does *not* dump
  energy into the node; it makes the node scatter along its own out-edges (carrying
  the step event) after its delay. Reuses the normal fire/scatter path + Ext 1
  payload. Native reference: `process_seed_at` pushes a `DelayedPropagation` onto the
  seeded neuron (`neural.rs:600-604`); `accepts_seed_track` (`neural.rs:324`) gates it.
- **Engine plumbing:** on each step-sequencer trigger from track T, inject a fire
  (carrying note/sample/vel) into every node whose *resolved* `seed-from` includes T,
  respecting that node's delay. The scheduler already has the hook sites
  (`process_seed_at` is called at the two seed sites the Phase-2 plan identified).

> Migration note: today's native neural *derives* its seed mask from `route` +
> output-override target tracks (`neuron_seed_track_mask`, `neural.rs:333`). The
> explicit `seed-from` (default `:route`) reproduces that default while removing the
> input/output conflation.

## 5. Execution model (engine)

A single per-block **discrete-event loop** over `[block_start, block_end)`, one
priority queue ordered by `(sample_time, node_index)`. Grid boundaries and synaptic
arrivals are both events in this queue.

```
on Deposit(amount, payload) into node n at sample-time t:
    lazy_leak(n, t)                 # decay leaky state by elapsed time since n last touched
    n.accum = reduce_n(n.accum, amount)
    n.in_event = payload            # Ext 1
    if n.resolution == :free:       # free-running: decide at arrival time
        try_fire(n, t)
    # else (grid): wait for n's scheduled Evaluate boundary

on Evaluate(n) at boundary t:       # grid nodes only, from per-node GridBoundaryClock
    lazy_leak(n, t)
    try_fire(n, t)
    n.accum = reduce_identity       # consume

try_fire(n, t):
    run n's compiled :update with (node-input, node-input-event, params, state) bound
    if it fired:
        schedule_audio_event(n, t, emit)            # :quantize :off keeps it free-timed
        fire_t = t + to_samples(n.delay)            # SOURCE-side delay (delay_steps)
        for e in n.out_edges:
            amt = gather_kernel(e, n)               # native; reads edge + src state
            enqueue Deposit(amt, payload) into e.to at fire_t + to_samples(e.delay)
```

Engine-owned details (all timing math, hence not lisp):
- **Grid vs free** differ in one branch: a deposit either triggers `try_fire`
  immediately (free) or only accumulates until the node's boundary (grid). v1 is
  **grid-only**; free-running is a later extension (it needs `:resolution :free` +
  the immediate-fire branch + elapsed-time `lazy_leak`).
- **delay** = `delay_steps` integer steps of the node's resolution (`.max(1)` floor —
  the one-tick latency floor, `neural.rs:601/878`). Later: free/float delay for
  sub-grid clusters.
- **double-buffer**: gather reads the *previous* tick's state; updates write a fresh
  buffer; swap. Deterministic, order-independent. The `(sample_time, node_index)`
  tiebreak handles simultaneous arrivals. Native reference: the `deferred_energy` /
  `deferred_source_events` two-phase apply (`neural.rs:670-734`).
- **backstop**: `MAX_EVENTS_PER_BLOCK`; on hit, `log()` the drop count (no silent
  truncation). Required once free-running recurrence exists; harmless before.
- **decay**: `energy *= energy_decay` at the network's finest grid step
  (`apply_energy_decay` / `finest_decay_index`, `neural.rs:764-780`). Model
  `:leak (per-step :energy-decay)` on the `energy` state field.

### Compile-once update bodies (critical perf note)

Tick mode today ships `:tick` as a **source string** re-evaluated each tick
(`RegisteredAccumulatorCallback::Source`, `lisp_host.rs:7572`). That is fine at low
tick rates but **will not scale** to per-node-event invocation. Graph mode must
**compile each `:update` (and `:gather`) once** at manifest-load time into reusable
bytecode/closures on the scheduler VM, then invoke with bound context per event.
Same publish channel (ship source), different scheduler-side handling (compile once,
invoke many).

## 6. Pipeline: compile → ship → build

1. **Compiler** (`eseqlisp/src/lang/compiler.rs`): extend the existing
   `def-sequencer` auto-quote (currently `:tick`/`:init` only, `compiler.rs:1511`,
   `1543`) to capture the **entire `def-sequencer` body** as a manifest. Use
   **auto-quasiquote** (not plain quote) so `,x` escapes to evaluation for computed
   config, and so it composes with the backtick/`,` macro form. `def-node` and
   `edges` are recognized as manifest grammar by head symbol — they are **not**
   registered callables.
2. **Publish channel** (existing): serialize the manifest via
   `eseqlisp::vm::format_lisp_source` and push through the `published_sequencers`
   channel on `SequencerState` (already built for tick mode). Upsert by stable id;
   bump version. Hot-reload reconciles by id (preserve runtime state on compatible
   edits).
3. **Scheduler** (`scheduler.rs`): on `published_sequencers_version` change, parse
   each manifest into a `GraphRuntime` (new) — see §8. Build is: materialize shape →
   instantiate nodes (defaults + per-instance plocks from the pattern store) →
   materialize topology into a sparse edge set → compile `:update`/`:gather` kernels
   → wire `seed-from` subscriptions. Reconcile by id at top of loop, never mid-chunk.

## 7. Implementation ordering (v1a → v1c)

Build in faithful increments; each is independently testable.

- **v1a — grid skeleton.** Single `def-node`, `:shape (grid 8 8)`, `(all-to-all)`,
  grid-aligned eval. Per-node `resolution`/`delay_steps`/`quantize` **editable and
  serialized from day one** (this is the groove surface; not deferrable). `energy +=
  gather; if energy >= threshold → fire, reset, decay; propagate after delay`. Nodes
  emit a fixed note (no payload relay yet). Reuses the existing per-neuron
  `GridBoundaryClock`, `delay_steps` scheduling, and `quantized_fire_timing` from
  `neural.rs`. **Acceptance:** a hand-wired weight matrix produces a stable spiking
  pattern; UI delay edits change the groove.
- **v1b — payload relay (Ext 1).** Seeds carry the step event; nodes relay +
  transpose. **Acceptance:** seed one node from the step sequencer; the seed note
  ripples through the net and re-pitches on each hop (feedback → evolving cascade).
- **v1c — faithful drop-in.** Add edge-state mutation/dampening (Ext 2),
  `seed-from`/`route` decoupling (§4), and engine `max-poly`/reset/seed (Ext 3).
  **Acceptance:** load a `def-sequencer` "neural", tune per-node delays in the UI, and
  hear it groove **identically to the native sequencer**; the native one can be
  retired or kept as a cousin.

> Out of scope for v1 (documented, not built): free-running `:resolution :free` +
> sub-grid float delay; multiple `def-node` prototypes / region assignment; topology
> generators beyond `all-to-all`; modular-cluster topologies; plasticity (mutable
> topology); arbitrary lisp `:gather` lambdas; the per-sequencer UI panel beyond what
> neural already renders.

## 8. Engine data structures (sketch)

New `crates/sequencer/src/sequencer/graph.rs` (sibling to `generator.rs`):

```
GraphRuntime {
    id, name,
    nodes:  NodeGroup,                 // SoA over the shape
    edges:  Vec<EdgeSet>,              // one per `edges` form; each holds sparse adjacency
    update_kernel: CompiledFn,         // per prototype (v1: one)
    gather_kernels: Vec<CompiledFn>,   // per EdgeSet
    queue: BinaryHeap<Event>,          // (sample_time, node_index)
    seed_subscriptions: TrackMask -> Vec<NodeIdx>,
    energy_decay, reset_beats, max_poly, max_poly_selection, ...
}

NodeGroup {                            // structure-of-arrays, per instance
    resolution: Vec<Timebase>, delay_steps: Vec<u32>, quantize: Vec<Option<Timebase>>,
    route: Vec<Option<usize>>, seed_from: Vec<SeedSource>,
    reduce: Reduce,                    // per prototype
    params: ParamTable,                // behavioral, with per-instance plocks
    state:  StateTable,                // runtime, leaky; double-buffered
    clocks: Vec<GridBoundaryClock>,    // reuse neural's per-node clock
}

EdgeSet { from_group, to_group, adjacency: SparseCsr, params: EdgeParamTable }  // weight, dampening, edge delay
```

Reuse from `neural.rs`: `GridBoundaryClock`, `next_grid_boundary`,
`quantized_fire_timing` semantics, `apply_energy_decay`/`finest_decay_index`,
`select_firing_candidates`, the deferred two-phase apply for double-buffering.

## 9. Serialization / plock store

Per-instance overrides ride the per-pattern channel that Phase 4b defines
(`ProjectSequencer` threaded through `ProjectPattern` → `PatternSnapshot` →
`SequencerSnapshot` + capture; grep `neural_networks` as the checklist). Keys:
- node intrinsic overrides: `(group, instance) -> {resolution, delay_steps, quantize, route, seed_from}`
- node param plocks: `(group, instance) -> {param -> value}`
- edge param plocks: `(group, from, to) -> {param -> value}` (sparse)
Defaults come from the `def-node`/`edges` prototype; the store holds only sparse
overrides. `#[serde(default)]` everywhere; round-trip test (old projects load empty).

## 10. Verification

1. `cargo test -p sequencer` — neural golden tests unchanged; new `graph.rs` unit
   tests (deterministic spiking over a known weight matrix; two nodes firing on one
   boundary order deterministically); `project.rs` round-trip for the new per-pattern
   store.
2. v1a/v1b/v1c acceptance tests as written in §7.
3. `run` skill: empty graph behaves identically to today (early-return guard); a
   hand-wired neural `def-sequencer` grooves and responds to UI delay edits and to
   step-sequencer seeds.

## 11. Key file references (existing code to reuse / extend)

- `crates/sequencer/src/neural.rs` — the algorithm being generalized:
  `ProjectNeuron`/`ProjectNeuralNetwork` (`:92`/`:114`), `process_seed_at` (`:591`),
  `process_boundaries_with_outputs` (`:633`), `apply_due_propagations` (`:782`),
  `firing_candidate` (`:827`), `commit_firing` (`:864`), `quantized_fire_timing`
  (`:923`), `apply_energy_decay`/`finest_decay_index` (`:764`), `GridBoundaryClock`
  (`:236`), `neuron_seed_track_mask` (`:333`).
- `crates/sequencer/src/lisp_host.rs` — `def-sequencer`/`register_sequencer_impl`
  (`:4294`/`:7534`), `RegisteredAccumulatorCallback::Source` (`:7572`), `seq-emit`
  builtin + `build_seq_emit_event` (`:4309`/`:7629`), the gen-* / state-* context
  builtins (`:4326`+).
- `crates/sequencer/src/generator.rs` — `GeneratorRuntime` (tick-mode runtime; model
  `GraphRuntime` as a sibling).
- `crates/sequencer/src/scheduler.rs` — publish-channel poll + reconcile; the
  `process_seed_at` seed sites; the velocity-merge + `max_poly` + `enqueue_network_
  trigger` path generators already use.
- `crates/eseqlisp/src/lang/compiler.rs` — auto-quote of `def-sequencer` `:tick`/
  `:init` (`:1511`, `:1543`) to extend to whole-body auto-quasiquote; macro expander
  (`:277`).
- `crates/sequencer/src/sequencer/state.rs` — `PublishedSequencer` + channel.
```
