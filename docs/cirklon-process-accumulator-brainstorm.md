# Cirklon-Style Step Processes: Design Spec

Status: design spec (evolved from brainstorm). Covers accumulators, masks, ratchets,
grabs, and the track-attached process chain, all built on the existing scheduler-owned
`def-process` framework.

## Design Principle

Port the Cirklon *evaluation model*, not the Cirklon surface constraints.

The durable idea:

```text
step-sequenced inputs + persistent state + ordered evaluation + typed target writes
```

Plus two verb families Cirklon expresses through its fixed aux-op menu, which we
express through Lisp:

```text
verdicts on the base event (veto/modify) + event multiplication (ratchets)
```

`def-process` already owns the right ownership split: Lisp declares behavior; Rust
owns scheduling, deterministic ordering, state persistence, and application to events.

## The Cirklon Model, Distilled

```text
step:       0 1 2 3 4 5 6 7
delta lane: 0 1 0 0 1 0 0 0
accum:      0 1 1 1 2 2 2 2
```

A normal parameter lane sequences the accumulator input. Accumulator state persists
across steps, so sparse lane values become a running musical transform.

Ideas to preserve:

- Persistent registers driven by sparse per-step lanes.
- Ordered evaluation: earlier operators mutate state that later operators read.
- Reset on transport start / pattern change; limit modes (clip, wrap, bounce).
- Masks gate reads/writes without resetting state — and state keeps advancing
  under a masked trig (load-bearing musically: the ramp continues under silence).
- Randomize/reset/threshold ops sequence *when to mutate*, not just what to output.
- Grab ops: one track/process reads another musical source.
- Ratchets in two forms: spawned sub-events and burst-local accumulators.

Ideas we deliberately do not copy:

- Hardcoded aux A/B/C/D as anonymous global step params.
- Every target as a MIDI-byte/CC-style value.
- Meaning encoded in slot position.
- Cross-track writes without explicit ordering rules.

Things we can do that Cirklon can't:

- Lanes named after their musical role (`prob-mask / prob`, not `aux_a`).
- Deterministic fold semantics → the UI can *preview* the accumulator curve,
  ghost-render ratchet sub-steps, and dim masked trigs before playback.
- Accumulator overflow as an event generator (`wrap-crash` below).
- History reads (`:steps-ago`) turning grabs into canons/delay-lines.

## Verified Repo Facts (as of this writing)

These ground the plan; all confirmed in code:

- `crates/sequencer/src/process.rs` — `ProcessInletDef` is name + default only
  (line ~34). `ProcessEventSource::TrackFires`/`SeqFires` exist but
  `matches_process_source` only matches `Channel`/`Outlet` (~line 66): track-step
  triggers parse but never fire.
- `crates/sequencer/src/lisp_host.rs` — `parse_process_inlets` (~4728) keeps only
  name + `:default`; the `:float 0 12` type/range users already write is silently
  discarded. Syntax precedent for metadata exists; it just isn't retained.
- `ProcessRunResult` outputs = outlets + emissions + `Option<f32>` transpose feeding
  a single global `ProcessRuntime.global_transpose`. Transpose is the one hardcoded
  typed target — the natural seam to generalize first.
- The synchronous invoke seam exists: `scratch.invoke_process_run(invocation)` in
  `scheduler.rs` (~4907) evaluates process Lisp synchronously inside the lookahead
  loop, with `apply_run_result` cascading follow-ups under
  `PROCESS_EVENT_CASCADE_LIMIT`. Step-chain evaluation reuses this, relocated into
  the per-track step-resolution path.
- `runtime_instance_id` falls back to a hash of the instance *name*
  (process.rs ~1097) — lane storage keyed on it breaks on rename. Lane data needs
  its own durable id or rename-aware migration in `sync_instances`.
- `EmittedAccumulatorEvent` carries `offset_beats` + full resolved step (chord,
  effect/instrument params); pending emissions are cleared on scene change and
  routed through `enqueue_due_process_emissions` *with* `midi_fx_quantizer_state`
  — ratchet sub-events have an existing downstream path.
- `sequencer/state.rs` — `state.pattern` already owns `effect_chains[track]`,
  `midi_fx_slots[track]`, `instrument_slots[track]`, `track_params[track]`.
  Chains-with-settings are already pattern-scoped; `process_chains[track]` follows
  the same model. `PublishedProcessDef` already flows into sequencer state.
- `ProcessRunInvocation` already has an `event: Option<Value>` slot — the hook for
  passing the resolved base event into step-attached runs.

## Core Architecture

### Replay safety: pure fold over the lane

ESeq resolves steps ahead of real time and can re-enter regions (scene switches,
pattern changes mid-lookahead, transport resets). Mutable once-per-fire accumulator
state is not replay-safe: a rescheduled chunk double-advances or skips.

For lane-driven accumulators, state is a **pure fold over the lane**:

```text
acc(step_n) = fold(op, reset_policy, lane[0..=n], cycles_elapsed)
```

This buys, simultaneously:

1. **Replay safety** — rescheduling recomputes the same value; no dedup keyed on
   `(pattern_epoch, step)`.
2. **Reset semantics for free** — transport start / pattern change = restart the
   fold at the cycle boundary.
3. **UI preview** — the step sequencer renders the computed accumulator curve under
   the sparse lane before anything plays.

Randomness stays pure via a process RNG seeded from `(process_id, cycle, step)`.
Seed policy is a musical knob:

- `:seed :per-cycle` — fresh rolls each loop (generative texture).
- `:seed :locked` — seed omits cycle: the same "random" result every cycle until
  reseeded (generative *composition*; "roll me a pattern, keep it").

Two-tier contract:

- `def-accumulator` / `def-mask` / `def-ratchet` sugar compiles to the pure-fold
  form: deterministic, previewable, replay-safe. Limit/wrap/bounce/reset and seeded
  randomize are all fold-expressible.
- Raw `def-process` with arbitrary `:run` keeps real mutable state and accepts
  fire-time-only semantics (no preview, best-effort under rescheduling).

If most library entries end up raw, that signals the sugar vocabulary is missing a
shape — not that authors love `set!`.

### Three verb families

Everything on the Cirklon aux-op menu collapses into these, combined with ordinary
Lisp conditionals:

1. **Target writes** — `(target-set! ...)` / `(target-add! ...)` on typed targets.
2. **Verdicts on the base event** — `(veto!)` masks this step's trig; mutator verbs
   modify it in place. The step-attached `:run` executes with the resolved base
   event as implicit context.
3. **Event multiplication** — `(ratchet! :times n :span dur :shape (fn (i ev) ...))`
   spawns sub-events **cloned from the base event** (chord, p-locks, param locks
   intact), never constructed from scratch. The `:shape` lambda with index `i` is
   the burst-local accumulator — a pure fold over `i`.

### Normative ordering rules

```text
track step fires
  1. resolve base step / p-lock params
  2. run attached step processes in chain order
  3. writes apply IMMEDIATELY after each process (aux-like ordered pipeline)
  4. surviving event + spawned events proceed to MIDI FX / graph / emission
```

- **Immediate ordered application.** Later processes read earlier writes — that is
  the Cirklon aux-ordering behavior in one sentence. Reads see pending writes from
  earlier processes in this step's chain. (The cascade loop in the scheduler is
  already this shape.)
- **Veto does not halt the chain.** Later processes still run; accumulator state
  keeps advancing under masked trigs. With pure folds this is automatic.
- **Spawned events go through the same downstream path** (MIDI FX, graph) as the
  base event.
- **Merge order: base → p-lock → process writes.** Manual p-locks are the base the
  process transforms (modulation-on-top, Cirklon mental model), not overridden by it.

### Typed targets with fused domains

A resolved target is `(address, domain)` — the domain rides with the target, and
dictates which ops are legal at definition time:

```rust
enum ParamTarget {
    StepParam { track: TrackRef, param: StepParam },
    TrackTimebase { track: TrackRef },
    TrackSwing { track: TrackRef },
    InstrumentParam { track: TrackRef, param: ParamRef },
    EffectParam { track: TrackRef, slot: usize, param: ParamRef },
    MidiFxParam { track: TrackRef, slot: usize, param: ParamRef },
    ProcessInlet { process: ProcessRef, inlet: String },
    ProcessChannel { name: String },
}
```

Domains: continuous range, ordered discrete (timebase), integer range, gate,
unordered enum. `add`/accumulate is legal only on ordered domains; enums get
`set`/`choose`/mask ops. Instrument/effect/MIDI-FX writes are authored normalized
0..1 and mapped through the target's domain metadata — authors never memorize
param units.

**`MidiFxParam` targets come early** (Phase 3, not late): a process lane driving an
existing repeat-MIDI-FX's `times` param per step is the cheapest route to a large
chunk of the generative vocabulary, before `veto!`/`ratchet!` verbs exist. Two
legitimate routes to a ratchet:

- self-contained: step process with `ratchet!` (one coherent authored behavior)
- compositional: existing MIDI FX + a process lane writing its params per step

### Lane-backed inlets

`ProcessInletDef` grows metadata: `kind`, `min`, `max`, `default`, `lane`, `doc`.
The `:in` declaration does triple duty — behavior input, UI surface, preset schema:

- `:lane true` inlets → lanes in the step-param sequencer, named
  `process-name / inlet-name`, range/default driving lane rendering.
- non-lane inlets → knobs in the process slot editor (like effect params).
- name + `:doc` → the library browser entry.

Lane storage is keyed by `(instance durable id, inlet name)`, lives per-pattern
alongside `step_data`, and does NOT extend the fixed `StepParam` enum.

### Composition: the track chain is primary, patching is secondary

- **Per-track step-process chain** (vertical): an ordered list of process instances
  attached to a track — structurally the effects-chain idiom. Chain order = aux
  order. Composition happens implicitly through the ordered pipeline. This is the
  90% case and the only composition surface normal users need.
- **Patching / channels / outlets** (horizontal): cross-track grabs, global fill
  flags, one process feeding many tracks. Already exists (`AuthoredPatch`,
  channels); stays agent/Lisp territory for now. `send`/`out` also feed
  visualization widgets and other processes — processes should publish their
  running values (e.g. `(send :climb-value acc)`) cheaply.
- Attachment **implies** the trigger: chain-attached processes need no
  `:on (track-step :self)` clause. `:on`/`:every` remain for self-clocked and
  channel-listening processes.

### Pattern scoping and identity

Follows the existing pattern-state model (`effect_chains` precedent):

- **Identity is track-level; settings are pattern-level.** The durable instance id
  lives on the track. Each pattern stores that instance's knob values and lanes.
  Pattern switch swaps config wholesale (rides the existing snapshot swap — no new
  sync machinery, scene-switch perf path unchanged).
- Reset policy decides whether accumulator state survives a pattern switch.
  Default: reset on pattern change (Cirklon behavior). `:reset :never` gives
  long-form builds that climb across patterns — possible only because identity
  outlives the pattern.
- "Pattern 2 has no ratchet" needs no feature: per-pattern chains either omit the
  process or leave its lane at the inert default. Track-level identity means
  removing it from one pattern doesn't orphan others' data.

### Presets

Three explicit granularities, all musically real:

1. **Process preset** — class + knob settings, no lanes ("prob-mask / locked
   seed"). Loads into the current pattern's slot. The browser-facing tier.
2. **Preset → all patterns** — explicit apply-everywhere gesture (avoids the
   Elektron "changed the kit, only this pattern heard it" confusion).
3. **Preset with lanes** — settings + lane content ("triplet stutter" includes the
   ratchet lane). The drop-a-groove tier; this is what makes the library feel like
   content rather than utilities.

Reuses the existing preset-bank format/conventions from instruments.

### Defaults-inert discipline

Every process must be audibly inert at defaults (`prob` = 1, `times` = 0,
`delta` = 0). Attaching never breaks the groove; sequencing the lane brings it in.
Same discipline as builtin effects. Library review should enforce this.

## Grab / Read Family

```lisp
(target-add! (read (track 1 :transpose)))                       ; current value
(target-add! (read (track (in :source) :transpose :steps-ago 8))) ; history
(target-set! (read (process transpose-wander :value)))
(target-add! (if (> (read channel :density) 0.5) 12 0))
```

Sources: source step param; another track's current resolved value; another
track's value N steps ago (history buffer — turns a grab into a canon/delay-line);
process state/outlet; channel value; `(rand)` from the seeded process RNG.

Cross-track *reads* are safe. Cross-track *writes* (especially timebase) need the
dependency/ordering model and are deferred.

## Timebase (deferred, riskiest)

Track clocks precompute step boundaries; changing another track's timebase changes
*when* it fires, not just payload. Rules:

- First experiment: **same-track only, sampled at next step boundary only** — the
  process writes a pending clock param that the clock reads when computing its next
  boundary; already-computed boundaries are never touched.
- Timebase is an ordered discrete domain, not raw ticks.
- Cross-track timebase wants an explicit control dependency graph. Do not fake it
  as emitted note data. Nothing else gates on this.

## Authoring Ergonomics

### Examples (target syntax)

Classic accumulator, raw:

```lisp
(def-process transpose-climb
  :doc "Sparse lane deltas accumulate into transpose, Cirklon-style."
  :target (step-param :transpose)
  :in ((delta :float -12 12 :default 0 :lane true)
       (reset :gate :default 0 :lane true)
       (limit :float 1 24 :default 12))
  :state ((acc 0))
  :run (do
    (if (gate? (in :reset))
        (set! acc 0)
        (set! acc (wrap (+ acc (in :delta)) 0 (in :limit))))
    (target-add! acc)))
```

Same thing, sugared (pure fold ⇒ UI curve preview):

```lisp
(def-accumulator transpose-climb
  :target (step-param :transpose)
  :amount (delta :float -12 12)
  :reset  :lane
  :range  (0 12)
  :mode   :wrap)
```

Probability mask:

```lisp
(def-process prob-mask
  :doc "Roll against a per-step probability lane; veto the trig on failure."
  :in ((prob :float 0 1 :default 1 :lane true))
  :seed :locked
  :run (when (> (rand) (in :prob))
         (veto!)))
```

Ratchet with velocity decay:

```lisp
(def-process repeater
  :in ((times :int 0 8 :default 0 :lane true)
       (decay :float 0 1 :default 0.7)
       (spread :float 0.5 2 :default 1))
  :run (when (> (in :times) 0)
         (ratchet! :times (in :times)
                   :span (* (step-length) (in :spread))
                   :shape (fn (i ev)
                            (vel! ev (* (vel ev) (pow (in :decay) i)))))))
```

Multi-target (named targets once a process writes more than one place):

```lisp
(def-process climb-and-open
  :targets ((pitch  (step-param :transpose))
            (cutoff (instrument-param :self :cutoff)))
  :in ((delta :float 0 2 :default 0 :lane true))
  :state ((acc 0))
  :run (do
    (set! acc (clip (+ acc (in :delta)) 0 24))
    (target-add! :pitch (floor acc))
    (target-set! :cutoff (/ acc 24))))       ; normalized; domain maps to range
```

Grab / call-and-response:

```lisp
(def-process echo-track
  :target (step-param :transpose)
  :in ((source :track :default 0)
       (lag :int 0 16 :default 8)
       (amount :float 0 1 :default 1 :lane true))
  :run (target-add!
         (* (in :amount)
            (read (track (in :source) :transpose :steps-ago (in :lag))))))
```

Threshold as event generator (accumulator overflow fires another track):

```lisp
(def-process wrap-crash
  :target (step-param :transpose)
  :in ((delta :float 0 4 :default 0 :lane true))
  :state ((acc 0))
  :run (do
    (set! acc (+ acc (in :delta)))
    (when (>= acc 12)
      (set! acc (- acc 12))
      (emit :track 7 :note 0 :vel 0.9 :duration 0.5))
    (target-add! acc)))
```

### Language-level requirements

- **Arithmetic helpers as builtins**: `wrap`, `clip`, `bounce`, `gate?`, `pow`,
  and friends (dgenlisp already has some of these — mirror that vocabulary in the
  process VM). Hand-rolled modulo-with-negatives is the classic authoring bug.
- **Verify `floor` in the process VM** (eseqlisp, not dgen — the dgen no-op bug may
  not apply here, but agents will reach for `floor` by reflex; verify or lint).
- **`(rand)`** (not `rng`) draws from the process RNG under the instance's `:seed`
  policy; authors never touch seeding.
- **Auto-bound inlet names in `:run`** — inlets bound as plain names so
  `(+ acc delta)` works; `(in :name)` stays for dynamic access. Cuts a third of the
  tokens in typical bodies.
- **Event handle verbs**, not raw maps: `(vel ev)`, `(vel! ev x)`, `(note! ev n)`,
  `(nudge! ev beats)` — authors don't depend on event internals.
- **`:doc` on every library process** — it is the browser entry.
- Bodies should stay 3–10 lines; the framework owns triggering, persistence,
  reset, RNG. Enforce clause ordering by convention/formatter.

The audit test for every library entry: *can you reconstruct what the track will do
from the `def-` form alone, without playing it?* Sugar forms must always pass.

## UI Surface

- Track → processes section: ordered chain of slots (effects-chain idiom), with
  add-from-browser, reorder (order is semantic), remove, per-slot knobs.
- Step-param sequencer (cirklon view) gains dynamic lanes per lane-backed inlet.
- Preview rendering (pure-fold processes only): computed accumulator curve drawn
  under its sparse input lane; ghost steps for ratchet sub-events; dimmed steps for
  masked trigs (exact with `:seed :locked`).
- Process library browser: builtins + user library, driven by manifests
  (name/doc/inlets), preset tiers 1–3.

## Implementation Plan

The gating architectural decision — pure-fold vs. mutable state under lookahead —
is baked in from Phase 1, not discovered late; it is the one choice that is
expensive to reverse.

### Phase 0 — Groundwork (no behavior change)

1. Extend `ProcessInletDef` with `kind`, `min`, `max`, `lane`, `doc`; retain them in
   `parse_process_inlets` (lisp_host.rs) instead of discarding. Mirror into
   `PublishedProcessInletDef`.
2. Add arithmetic builtins (`wrap`, `clip`, `bounce`, `gate?`) to the process VM
   env; verify `floor`; add `(rand)` backed by a seeded per-invocation RNG (seed =
   hash(process_id, cycle, step) per `:seed` policy; policy parsed on `def-process`).
3. Introduce a durable instance id independent of the name hash (or rename-aware
   migration in `sync_instances`); this is the lane-storage key.
4. Auto-bind inlet names in `:run` compilation.

### Phase 1 — Track-step invocation (the first slice)

1. Wire `ProcessEventSource::TrackFires`: scheduler publishes track-step fires into
   `ProcessRuntime`; extend `matches_process_source`.
2. Add minimal chain attachment: one process instance attachable to a track
   (authoring call + snapshot plumbing), invoked synchronously in the per-track
   step-resolution path via the existing `invoke_process_run` seam, *before* the
   event payload is finalized. Invocation carries source track, step index, cycle
   count, resolved base params in the existing `event` slot.
3. Per-pattern dynamic lane store keyed `(instance id, inlet name)` beside
   `step_data`; lane value resolution feeds `(in :name)` for `:lane true` inlets.
4. One typed target: generalize the hardcoded `transpose` seam to a per-track,
   step-scoped `StepParam::Transpose` write composing base → p-lock → process.
5. Reset semantics: fold-based state derivation for the accumulator path; reset on
   transport start / pattern change.
6. **Acceptance test** (integration): the sparse example —
   `delta: 0 1 0 0 1 0 0 0` ⇒ `acc: 0 1 1 1 2 2 2 2` ⇒ output = base transpose +
   acc — including a reschedule/scene-switch replay test proving no double-advance.

### Phase 2 — UI lanes and slots

1. Publish attached instances + lane-backed inlet manifests to the UI.
2. Render dynamic lanes in the cirklon step-param view (name, range, default).
3. Process slot editor for non-lane inlets (effects-param idiom, ui/effects.rs as
   reference).
4. Library browser entry for process defs (name + doc).
5. Defaults-inert check: attaching at defaults is audibly a no-op.

### Phase 3 — Typed targets, MIDI FX early

1. `ParamTarget` enum with fused domain metadata; normalized-write mapping.
2. `MidiFxParam` and `InstrumentParam`/`EffectParam` targets. Milestone: a process
   lane driving an existing repeat-MIDI-FX's `times` per step (compositional
   ratchet, before `ratchet!` exists).
3. Multi-target syntax (`:targets` + named `target-set!`/`target-add!`).
4. Define and test merge order vs p-locks per target kind.

### Phase 4 — Verdicts and ratchets

1. `(veto!)`: verdict in the run result; event marked dead; chain continues; state
   still advances. Dimmed-step preview for `:seed :locked` masks.
2. `(ratchet! :times :span :shape)`: clones the base event (chord/p-locks/params
   intact), applies the `:shape` fold per sub-event, schedules via the pending-
   emissions store; sub-events routed through the existing
   `enqueue_due_process_emissions` / MIDI FX path. Ghost-step preview.
3. `prob-mask` and `repeater` land as builtin library processes with presets.

### Phase 5 — Ordered chains

1. Multiple processes per track, explicit order index; reorder/remove in UI.
2. Immediate ordered application; later processes read earlier pending writes;
   cascade limit reused.
3. Chain state moves into `state.pattern.process_chains[track]` (settings + lanes
   pattern-scoped; identity track-level).

### Phase 6 — Pattern scoping polish + presets

1. Track-level identity / pattern-level settings reconciliation across pattern
   switch; `:reset :never` carry-over.
2. Preset tiers 1–3 (settings / apply-to-all-patterns / settings+lanes) on the
   existing preset-bank format.

### Phase 7 — Grabs and reads

1. `read` expression family: track param (current + `:steps-ago` history buffers),
   process outlet/state, channel.
2. History buffer sizing/retention policy per track param.
3. `echo-track` and `wrap-crash` as library examples.

### Phase 8 — Sugar tier

1. `def-accumulator`, `def-mask`, `def-ratchet` expanding at authoring time
   (lisp_host) into pure-fold processes — runtime only ever sees `def-process`.
2. Pure-fold contract enables curve preview in the lane UI.
3. Revisit the library: raw entries that fit a sugar shape get rewritten; recurring
   raw shapes with no sugar indicate a missing form.

### Phase 9 — Timebase (last)

1. Same-track, next-step-boundary-only pending clock param.
2. Ordered-discrete domain ops for timebase values.
3. Cross-track timebase deferred until a control dependency graph exists.

## Locked Decisions

- Immediate ordered write application (not final-merge); reads see pending writes.
- Veto does not halt the chain; state advances under masked trigs.
- Merge order: base → p-lock → process writes.
- Identity track-level, settings/lanes pattern-level; default reset on pattern change.
- Chain attachment implies the trigger (no `:on` for chain processes).
- `(rand)` is the RNG verb; `:seed :locked | :per-cycle` on the instance.
- `MidiFxParam` targets early (Phase 3), timebase last (Phase 9).
- Ratchets clone the base event; sub-events go through the normal downstream path.
- Lane storage keyed by durable instance id, outside `StepParam::ALL`.
- Defaults-inert discipline for every library process.
- Patching/channels UI deferred; track chain is the user-facing composition surface.

## Open Questions

- Exact preset file layout for tier 3 (lane content serialization).
- History buffer depth and memory policy for `:steps-ago` reads.
- Whether `def-mask`/`def-ratchet` sugar shapes are right, or one `def-step-fx`
  umbrella reads better — decide after the first few library entries.
- Per-pattern process *bypass* toggle vs. relying on inert lane defaults.
- How process `send`/outlet values surface in visualization widgets (scope/meter
  lane?) — cheap win, design later.
