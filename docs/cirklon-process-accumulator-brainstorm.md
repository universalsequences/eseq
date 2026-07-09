# Cirklon-Style Step Processes: Design Spec

Status: design spec (evolved from brainstorm). Phase 4 backend verdicts,
ratchets, and process-inlet connections have landed. Rack-slot write
application and the Phase 5 ordered-chain editing surface remain follow-ups.
Covers accumulators, masks, ratchets, grabs, and the track-attached process
chain, all built on the existing scheduler-owned `def-process` framework.

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

## Verified Repo Facts (re-verified through Phase 3A, 2026-07-08)

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

### Landed since the original draft (affect this design)

- **Racks** (`RackSlotParam`, `RackSlotSnapshot`, `RackSlotParamPlocks` in
  `sequencer/state.rs`): tracks are no longer one-instrument. Any target model
  that says `InstrumentParam { track, param }` is already stale — see the rack
  addressing note under Typed Targets.
- **P-lock variants** (`plock_variants.rs`, commit `8d065f39`): interned p-lock
  bundles keyed on exact `value_bits`, with domains split
  `Instrument / Effect / RackSlotParam / RackSlotInstrument(Tensor)`. Two
  consequences: process writes must never persist into plock/step storage
  (would corrupt variant identity — now a locked decision), and the merge-order
  claim must be validated against the reworked step-resolution path
  (`state_values.rs` grew ~3k lines across `8d065f39`/`207e4589`).
- **Key-locks spec** (`docs/key-locks-spec.md`, draft): per-note param
  overrides resolved at voice-assignment time on the *sounding* pitch. Defines
  its own resolve chain that must compose with process writes — see Merge
  Order below. Key-locks is the smaller feature and shares the resolve seam;
  prefer landing it first so this work builds on a settled chain.

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
3. **Event multiplication** —
   `(ratchet! :times n :mode :subdivide|:repeat :span dur :shape (fn (i ev) ...))`
   spawns sub-events **cloned from the base event** (chord, p-locks, param locks
   intact), never constructed from scratch. The `:shape` lambda with index `i` is
   the burst-local accumulator — a pure fold over `i`. `:mode` selects the two
   Cirklon-style repeat behaviors and reinterprets `:span` accordingly (see
   Phase 4 for the full table).

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
- **Merge order (full chain, per param):**
  `patch default → key lock → step p-lock → process write → mods`. Manual
  p-locks are the base the process transforms (modulation-on-top, Cirklon
  mental model), not overridden by it; key locks (per-note overrides,
  `docs/key-locks-spec.md`) sit below step p-locks; mods modulate around
  whatever survives. This is the one authoritative ordering — both specs cite
  it.
- **Process writes are transient fire-time overlays.** They are never written
  back into plock/step storage. This was always implied; `plock_variants.rs`
  makes it load-bearing — variants intern p-lock bundles on exact `value_bits`,
  and a persisted process write would corrupt variant identity.
- **Process transpose writes change the sounding pitch, which changes which key
  lock fires.** Processes run at step-resolution time, before voice
  assignment, so the key-lock lookup naturally sees the post-process pitch.
  Correct behavior, but it must be an explicit test in whichever feature lands
  second.

### Typed targets: ports (def-time) vs. bindings (instance-time)

Portability requirement: a process must "just work" when dropped on an arbitrary
project. A def that hardcodes `(instrument-param :self :cutoff)` or an effect
slot index only works on tracks that happen to match its authoring project. So
the target model is split in two:

- **Ports** — the `:targets` clause in a `def-process` declares named
  *output ports*, **not addresses**. `target-set!` / `target-add!` write to
  ports, never to concrete params. A port has one explicit binding mode:
  fixed/hint-following, parameter-mappable, or process-connectable.
  - `(name target-hint)` — fixed/hint-following port. It may resolve
    automatically, but it is not shown as remappable in the UI. This is the
    right shape for things like `(step-param :transpose)`.
  - `(name :mappable)` — no default hint; starts unbound and is assigned in
    the mapping UI.
  - `(name :mappable target-kind)` — mappable unknown target constrained by
    compatibility class, e.g. `:device-param`, `:step-param`,
    `:instrument-param`, `:effect-param`, or `:midi-fx-param`.
  - `(name :mappable target-hint)` — mappable target with a default selector,
    e.g. `(param-tag :cutoff)` or `(instrument-param :release)`.
  - `(name :process-inlet)` — connectable only to another process instance's
    declared inlet. It is wired with `connect!`/`:connect` and never appears in
    the parameter-mapping UI.
- **Bindings** — each chain *instance* stores `port → Option<ParamTarget>`:
  the concrete address, chosen per-project. Fixed/hint-following ports resolve
  from their hint only; parameter-mappable ports may be manually assigned to
  compatible parameter targets; process-connectable ports accept only
  `ProcessInlet` targets. A def may carry a *default binding hint* — a selector,
  not an address — auto-resolved when the process is attached (and re-tried when
  the chain changes while the port is still on its hint, i.e. the user hasn't
  manually rebound):
  - step-param hints (`:transpose`, velocity…) bind unconditionally — every
    track has them; this is why the portable library examples all target them.
  - `(param-tag :cutoff)` — semantic tag match across the track's
    instrument/effect/MIDI-FX params. Params opt in: an instrument's param
    descriptor can declare tags (the Prophet-style ladder filter's `frequency`
    param tags itself `:cutoff`), so hints work without knowing any device's
    literal param names. Falls back to exact-name match when no tag matches,
    so untagged devices still resolve when names happen to line up.
  - `(midi-fx-target :beat-repeat :gate)` — kind-selector: "if this track's chain
    has beat-repeat, drive its `gate`"; matches on the builtin's kind id,
    first matching slot wins. This is the "opportunistic" authoring style:
    the process does something extra on tracks that happen to have the gear.
    A future true repeat-count param can use the same shape with `:times`.
  - `:mappable` with no hint — the port starts unbound: the "general-purpose
    accumulator on a mappable parameter" style, bound entirely through the
    mapping UI.

**Param tags (small curated vocabulary, not an ontology).** Tags live on the
param descriptor at the device level — instruments declare them in their param
metadata, builtin effects in their descriptors — and are the polymorphic layer
that makes hints portable: any device answering to `:cutoff` receives
cutoff-targeting processes, regardless of what it calls the knob. Keep the
vocabulary short and reviewed (`:cutoff`, `:resonance`, `:decay`, `:drive`,
`:wet`, `:feedback`, `:rate`, `:depth`, `:times`…) — a tag is a promise about
musical role, so additions go through the same library review as
defaults-inert. One param may carry multiple tags; ties resolve
instrument-first then chain order, first match wins. The same tags double as
the "mappable parameter" filter in the mapping UI (arm a mappable
`:cutoff`-hinted port and tagged params highlight brightest).

**Soft resolution (locked):** bindings resolve at fire time against live
pattern state. An unresolvable or unbound port makes that write a silent no-op —
the process still runs, state still advances, other ports still apply. The UI
flags the binding as unbound/stale; nothing errors. This is the write-side twin
of the defaults-inert discipline, and mirrors the stale-target handling already
specified in `MACRO_MAPPING_SPEC.md` §7.

**Binding scope:** bindings live with track-level *identity*, not pattern-level
settings — "this climb drives *that* filter" is part of what the process is on
this track. (Per-pattern binding overrides are a possible later addition, not
the default.)

**Parameter-mapping UI + shared seam with macros:** parameter binding should be performed with the
arm-and-highlight interaction specified in `MACRO_MAPPING_SPEC.md` (draft, not
yet implemented). The mapping affordance is not an inlet property. Instead,
when the selected process lane belongs to process slot X, the lane UI shows all
`:mappable` parameter ports on slot X. Fixed targets, including fixed
`(step-param :transpose)` targets, do not show map controls. Pressing a map
button arms mapping mode, every compatible param highlights (a third wrapper
color — modulation blue, macro green, process ports their own), click to bind.
The macro spec's `MacroTarget` identity machinery (`ParamNodeId` / `logical_id`
guards so bindings survive node rebuilds, re-resolve-and-flag on load) is
exactly the runtime half of `ParamTarget`. Phase 3B should build the arm-mode
wrapper fork as shared infrastructure rather than inventing a process-only
mapping path.

This also fixes the preset story: tier-3 presets ship ports + hints, never
addresses. Dropped on any project, fixed step-param ports bind immediately and
the preset grooves; mappable instrument/effect ports may start from a hint but
remain user-assignable, and hintless mappable ports sit unbound-but-inert until
arm-mapped.

`ParamTarget` remains the *address* type a binding resolves to — `(address,
domain)`, where the domain rides with the target and dictates which ops are
legal:

```rust
enum ParamTarget {
    StepParam { track: TrackRef, param: StepParam },
    TrackTimebase { track: TrackRef },
    TrackSwing { track: TrackRef },
    InstrumentParam { track: TrackRef, instrument: InstrumentRef, param: ParamRef },
    EffectParam { track: TrackRef, slot: usize, param: ParamRef },
    MidiFxParam { track: TrackRef, slot: usize, param: ParamRef },
    RackSlotParam { track: TrackRef, slot: usize, param: ParamRef },
    RackSlotInstrumentParam { track: TrackRef, slot: usize, param: ParamRef },
    ProcessInlet { process: ProcessRef, inlet: String },
    ProcessChannel { name: String },
}
```

### Process-inlet targets: connecting within the chain (Phase 4)

The Cirklon aux-A-feeds-aux-D pattern — one process generates a value another
process consumes (a dice roll driving a ratchet's `times`) — is a small delta
on the model, not a new model. The locked immediate-ordered-application rule
already defines the semantics: a write from slot N lands before slot N+1 runs,
so a `ProcessInlet` write is same-tick and order-visible within one track's
chain. The cross-track one-tick register rule does not apply inside a chain.

Rules:

- **Defs stay generic.** A `:process-inlet` port is declared shape-only:
  `(out :process-inlet)` — a connectable port kind, never a parameter-mappable
  port and never a hint naming
  another process class. A def that names a sibling class couples two
  blueprints (worse than `midi-fx-target`, whose target is a stable device).
  The generator doesn't know if it's driving a ratchet count, a mask
  probability, or a climb delta.
- **Wiring is instance-level**, stored in `TrackProcessSlot.bindings` like any
  other port binding. Two authoring surfaces, both writing the same store:

  ```lisp
  ;; handles + connect! — the live-coding surface (parallels lane!)
  (def rng (dice :lo 1 :hi 4))
  (def rep (repeater :decay 0.7))
  (processes :track 0 rng rep)
  (connect! rng :out (inlet rep :times))

  ;; inline selector — the declarative one-block surface
  (processes :track 0
    (dice :lo 1 :hi 4 :connect '((out (process-inlet :repeater :times))))
    (repeater :decay 0.7))
  ```

  The inline `:connect` value is quoted because it is declarative selector data.

  `(process-inlet :class :inlet)` is a selector, not an address — it resolves
  against the finished chain (first matching slot), which is why the inline
  form works despite `processes` being whole-chain-replace: a forward handle
  reference to a sibling two lines down doesn't exist yet at read time, a
  selector does. `connect!` and the Phase 3B parameter-mapping gesture write
  the same instance binding store, but are deliberately different authoring
  operations with different validation and UI surfaces.
- **Write application**: the write lands as a transient overlay on the target
  slot's inlet resolution for this fire — never persisted, composing with the
  inlet's lane per the op (`target-set!` owns the value and the lane dims in
  the UI; `target-add!` embellishes the hand-authored lane).
- **Chain position matters**: a write to an *earlier* slot's inlet is only
  seen on the next fire. This is Cirklon-authentic (aux order is evaluation
  order) and the UI already treats chain order as semantic; no cycle handling
  needed.
- **Soft resolution as usual**: no matching slot ⇒ silent no-op, stale badge.
  A generator attached alone is inert (defaults-inert holds).
- **Fan-out is the channel layer's job.** A port binds to one inlet. One
  generator driving many consumers publishes a channel (`(send :dice held)`)
  and consumers read it (Phase 7) — ports are patch cords inside one track's
  chain (same-tick, previewable, preset-portable); channels are the
  project-wide bus (one-tick latency).
- **Preview caveat**: a pure-fold consumer driven by a process-inlet write is
  previewable only if the producer is also pure-fold — the composed fold is
  still deterministic, but Phase 8's preview must evaluate the chain prefix,
  not one lane in isolation.
- **Preset granularity**: the wire is chain-level state, so tier-1 (class +
  settings) presets cannot capture a patch. "Random ratchets" is a *chain
  preset* (both slots + the binding) — a granularity the preset tiers don't
  have yet; see Open Questions.

**Rack addressing.** Tracks are no longer one-instrument: racks landed
(`RackSlotSnapshot` etc. in `sequencer/state.rs`), and `plock_variants.rs`
already had to split its domains into
`Instrument / Effect / RackSlotParam / RackSlotInstrument(Tensor)`.
`InstrumentRef` above must mirror that split (plain slot vs. rack slot vs.
instrument-inside-rack-slot) from the first version of the enum — this is
part of the expensive-to-reverse decision, not a Phase 3 detail to retrofit.
On a rack track, `:self` in an instrument-param target means *the slot the
base event routes to*. Phase 3A added rack-aware `ParamTarget` variants to keep
the data model honest, but the scheduler intentionally treats rack-slot process
writes as no-ops until the rack audio dispatch path can apply them transiently
and be tested end-to-end.

Domains: continuous range, ordered discrete (timebase), integer range, gate,
unordered enum. `add`/accumulate is legal only on ordered domains; enums get
`set`/`choose`/mask ops. Instrument/effect/MIDI-FX writes are authored normalized
0..1 and mapped through the target's domain metadata — authors never memorize
param units.

**`MidiFxParam` targets come early** (Phase 3, not late): a process lane driving
an existing repeat-style MIDI-FX param per step is the cheapest route to a large
chunk of the generative vocabulary, before `veto!`/`ratchet!` verbs exist. With
ports, this is a port hinted `(midi-fx-target :beat-repeat :gate)` today because that
is an existing tested `beat-repeat` param; a true repeat-count/`times` param
belongs with the MIDI-FX itself, not as a fake process target. The port lights up
on any track carrying that FX and no-ops elsewhere. Two legitimate routes to a
ratchet:

- self-contained: step process with `ratchet!` (one coherent authored behavior)
- compositional: existing MIDI FX + a process lane writing its params per step

### Phase 3A backend checkpoint (landed)

The first backend slice is intentionally hint-driven, not UI-mapped yet:

- `ProcessDef` / `ProcessRunInvocation` now use `ProcessPortDef` entries instead
  of a single internal target. Legacy `:target (...)` is normalized into one
  internal default port; `:targets ((pitch (...)) (gate (...)))` declares named
  ports.
- `target-set!` / `target-add!` accept either `(target-set! value)` for the
  sole/default port or `(target-set! :port value)` for a named port. Step-param
  writes keep raw musical units; instrument/effect/MIDI-FX writes are normalized
  0..1 and mapped through descriptor metadata at fire time.
- Supported hints: `(step-param :name)`, `(param-tag :tag)`,
  `(instrument-param :name)`, `(effect-param :effect :param)`, and
  `(midi-fx-target :fx :param)`. Quoted legacy `(fx-param :fx :param)` /
  `(midi-fx-param :fx :param)` forms still parse, but unquoted process target
  helpers must not reuse live MIDI-FX API names. Tag matching is tag-first with
  exact-name fallback.
- The scheduler accumulates a transient process overlay while evaluating the
  ordered track chain. Step-param writes apply immediately to the in-flight
  `ResolvedStep`; effect/instrument writes upsert into the scheduled event
  payload after defaults and p-locks; MIDI-FX writes apply only to the temporary
  slot snapshot passed into MIDI-FX invocation.
- Sampler core params that live in both generic instrument params and structured
  trigger params are kept coherent at the final enqueue boundary. This is why a
  process write to `(instrument-param :speed)` now changes audible sampler
  playback speed instead of only appearing in `instrument_params`.
- Stale or unbound targets are soft no-ops. MIDI-FX bindings use slot + FX name +
  param name guards because MIDI-FX slots do not have audio-node `ParamNodeId`s.
  Effect and ordinary instrument params use node identity where available.
- Rack target variants exist in the data model, but rack-slot writes are not
  exposed as supported behavior until the rack audio dispatch path applies them
  transiently and has deterministic coverage.
- No process write persists into step data, p-locks, key-locks, defaults, or slot
  snapshots. Tests cover p-lock storage staying unchanged, ordered chain writes,
  stale MIDI-FX no-op behavior, MIDI-FX temporary slot overrides, and transient
  instrument/effect payload writes.

The demo script `crates/sequencer/scripts/process-phase3a-ports-demo.lisp`
exercises the current backend surface: named `pitch`, `gate`, and `speed` ports
attached to track 0, with live handle updates like
`(phase3a-port-writer-h :pitch 4)` and `(phase3a-port-writer-h :speed 0.75)`.
For sampler `speed`, remember the process value is normalized: `0.625` maps to
raw speed `1.0`, `0.75` maps to raw `2.0`, and `1.0` maps to raw `4.0`.

### Phase 3B landed: mapping UI and binding visibility

The backend now has enough behavior to make UI the highest-yield next step:

- `:mappable` is a target-port declaration, not an inlet declaration.
  Non-mappable ports are fixed/hint-following and do not expose map controls.
- Process slot rows should not wholesale list every target port. When a selected
  step lane belongs to process slot X, the lane UI shows mapping widgets for all
  mappable target ports on slot X, including current hint, resolved target, and
  stale/unbound status.
- Arm a mappable process port using the same parameter wrapper seam as
  `MACRO_MAPPING_SPEC.md`; compatible params highlight, tag/name-hint matches
  rank highest, click binds.
- Manual bindings write `TrackProcessSlot.bindings[port] = Some(ParamTarget)`;
  unbind returns the port to hint-driven auto-resolution or explicit unbound
  state, depending on the chosen UX.
- Project scratch re-evaluation must not erase restored pattern-owned slot state:
  if the same process instance/class is reattached, saved lane edits and manual
  port bindings are preserved. Clearing the chain or creating a new instance is
  the explicit reset path.
- Stale badges come from the same fire-time resolution rules as the scheduler:
  no hard errors, no state mutation, other ports keep applying.
- The process color should be distinct from macro/modulation colors but reuse the
  same arm/highlight infrastructure rather than building a parallel mapping mode.

Phase 3B should not try to solve rack-slot writes. The UI may display rack-aware
target types as unavailable/unsupported until Phase 3C below.

### Phase 3C deferred: rack write application

Before claiming rack targets are supported:

- Thread process target overlays through the rack audio dispatch path.
- Define exactly how a rack slot is selected for `:self` on routed/chord events.
- Apply rack slot params and rack-slot instrument params transiently, without
  mutating rack p-locks/defaults/snapshots.
- Add deterministic scheduler/audio-dispatch tests proving rack writes affect the
  temporary event payload only.

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

### Attachment authoring form (landed decision)

The Lisp surface for attaching processes to tracks — usable today for testing,
and the same surface the UI will drive later. Declarative for structure,
handles for live tweaking:

```lisp
;; Declare a track's whole chain (ordered). Replaces any existing chain —
;; re-evaluating the buffer is idempotent.
(def climb
  (processes :track 0
    (transpose-climb :limit 12
                     :delta (lane 0 1 0 0 1 0 0 0))))

(processes :track :all (prob-mask :prob (lane 1 0.75)))  ; every track
(processes :track (list 0 3) ...)                        ; a set of tracks
(processes :track 0)                                     ; empty = clear

;; Handles keep the existing instance-call idiom alive after attachment:
(lane! climb :delta 0 2 0 0 2 0 0 0)   ; re-sequence a lane
(climb :limit 6)                        ; knob tweak (write-through: follow-up)
```

Rules:

- Instance forms are ordinary class calls (`(transpose-climb :limit 12)`) —
  the existing instantiation idiom. `processes` consumes the resulting
  handles and turns them into chain slots.
- **Lane-backed inlets accept either a scalar (constant) or a `(lane ...)`
  literal** — the call site mirrors the `:in` declaration's triple duty.
  `(lane ...)` on a non-lane inlet is an error. Steps beyond the lane's
  length read the inlet default.
- Chain attachment implies the trigger (no `:on`), per the locked decision.
- Writes go to the *current pattern's* chain (settings/lanes pattern-scoped,
  identity track-level). `:patterns :all` can come later with preset tier 2.
- `processes` returns the handle when given one instance, a list otherwise.

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

### History semantics: sample-and-hold registers

Reads return **resolved** values (post p-lock, post process writes — including the
source track's own accumulators), not lane/pattern data. Each readable track param
has a "last resolved value" register updated when a trig fires; the history buffer
records what the register *held* at each step boundary, not what fired there.

- `:steps-ago 8` = the register's state 8 steps ago. If the last trig was 9 steps
  ago and nothing fired at step N-8, you get that trig's value (the note still
  "ringing" is the track's pitch state). Gaps never produce nil.
- `:trigs-ago n` = count fired events, not grid steps — "the previous note it
  played" for sparse sources. `:steps-ago` gives time-locked canons; `:trigs-ago`
  gives call-and-response that follows the source's phrasing.
- Before anything has fired, the register holds the param's base value (0 for a
  transpose add) — grabs are inert until the source has played. Consistent with
  defaults-inert.
- **Determinism rule:** history reads see register state as of the end of the
  *previous* step. Current-tick cross-track values are visible only through
  explicit chain ordering, never implicitly — no same-step resolution races.
- Registers/history follow the accumulator reset policy (clear on pattern change
  by default; a pattern-surviving echo is a `:reset :never` variant, not default).

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
  :in ((times  :int 0 8 :default 0 :lane true)
       (mode   :enum (:subdivide :repeat) :default :subdivide)
       (decay  :float 0 1 :default 0.7)
       (spread :float 0.5 2 :default 1))
  :run (when (> (in :times) 0)
         (ratchet! :times (in :times)
                   :mode  (in :mode)   ; :subdivide fills the step; :repeat trails past it
                   :span  (* (step-length) (in :spread))
                   :shape (fn (i ev)
                            (vel! ev (* (vel ev) (pow (in :decay) i)))))))
```

`mode` is an ordinary inlet, so the choice is a knob (and lane-/p-lockable like
everything else). Authors writing raw `:run` bodies can also pass `:mode`
directly, even computed per-fire (`:mode (if (in :long) :repeat :subdivide)`).

Multi-target (named ports once a process writes more than one place; fixed
ports use their hint only, while `:mappable` ports expose the mapping UI — see
Ports vs. bindings):

```lisp
(def-process climb-and-open
  :targets ((pitch  (step-param :transpose))       ; fixed: no map UI
            (cutoff :mappable (param-tag :cutoff))) ; default selector + map UI
  :in ((delta :float 0 2 :default 0 :lane true))
  :state ((acc 0))
  :run (do
    (set! acc (clip (+ acc (in :delta)) 0 24))
    (target-add! :pitch (floor acc))
    (target-set! :cutoff (/ acc 24))))       ; normalized; domain maps to range
```

Opportunistic FX hint + fully generic mappable port. Selecting any lane owned
by this process slot shows the `aux` mapper; `gate` stays fixed/hint-following:

```lisp
(def-process repeat-gate-brain
  :targets ((gate (midi-fx-target :beat-repeat :gate))  ; binds only if the track has it
            (aux  :mappable :device-param))       ; no hint: map in the UI
  :in ((energy :float 0 1 :default 0 :lane true))
  :run (do
    (target-set! :gate energy)
    (target-set! :aux energy)))
```

Generator + consumer patch (process-inlet ports, Phase 4). The def is a
generic number source — the ratchet coupling lives entirely at the
attachment site; `repeater` is unchanged and doesn't know it's being driven:

```lisp
(def-process dice
  :doc "Roll an integer each fire and publish it."
  :targets ((out :process-inlet))   ; connectable shape only — no class named here
  :in ((lo :int 0 8 :default 1)
       (hi :int 0 8 :default 4)
       (roll :gate :default 1 :lane true))    ; sequence WHEN to reroll
  :seed :locked
  :state ((held 0))
  :run (do
    (when (gate? roll)
      (set! held (+ lo (rand-int (- hi lo)))))
    (target-set! :out held)))

(processes :track 0
  (dice :lo 1 :hi 4 :connect '((out (process-inlet :repeater :times))))
  (repeater :decay 0.7))
```

Repoint the same generator at a mask's probability and nothing about `dice`
changes — only the instance wiring:

```lisp
(connect! rng :out (inlet mask :prob))
```

Accumulator with a fixed target: no mapping UI, even though the amount inlet is
lane-backed.

```lisp
(def-accumulator sparse-transpose
  :target (step-param :transpose)
  :amount (amount :float -12 12 :lane true)
  :range (-24 24)
  :mode :clip)
```

Accumulator with a user-mapped target. `:target-kind` keeps normalized amount
values away from raw musical step params; `:target-hint` is optional and remains
a selector, not an address.

```lisp
(def-accumulator filter-rise
  :target :mappable
  :target-kind :device-param
  :target-hint (param-tag :cutoff)
  :amount (amount :float 0 1 :lane true)
  :range (0 1)
  :mode :clip)
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
   This test validates the gating pure-fold decision, so the replay harness is
   in-scope for Phase 1, not deferred: check whether the existing process tests
   in `lisp_host.rs` (~19386+) or the scene-switch perf work left a harness
   that can drive a scene switch mid-lookahead deterministically; if not, build
   one as part of this phase.

### Phase 2 — UI lanes and slots

1. Publish attached instances + lane-backed inlet manifests to the UI.
2. Render dynamic lanes in the cirklon step-param view (name, range, default).
3. Process slot editor for non-lane inlets (effects-param idiom, ui/effects.rs as
   reference).
4. Library browser entry for process defs (name + doc).
5. Defaults-inert check: attaching at defaults is audibly a no-op.

### Phase 3 — Ports, bindings, typed targets, MIDI FX early

#### Phase 3A — Backend ports and fire-time bindings (landed)

1. `ProcessDef` / `ProcessRunInvocation` use named `ProcessPortDef` entries.
   Legacy `:target` normalizes to one default port; `:targets` declares named
   ports.
2. `target-set!` / `target-add!` support default-port and named-port forms.
3. `ParamTarget` / binding data exists with stale-safe target variants:
   `StepParam`, ordinary `InstrumentParam`, `EffectParam`, `MidiFxParam`, and
   rack-aware variants that are present but not yet applied.
4. Fire-time hint resolution works for step params, instrument params, effect
   params, MIDI-FX params, and param tags with exact-name fallback.
5. Transient overlays apply step-param writes immediately; device writes upsert
   into scheduled event payloads after base defaults and p-locks; MIDI-FX writes
   affect only the temporary slot snapshot used for invocation.
6. Device-param writes are normalized 0..1; step-param writes keep raw musical
   units.
7. Deterministic tests cover named ports, stale MIDI-FX no-op behavior,
   transient p-lock-safe writes, MIDI-FX temporary overrides, and sampler
   instrument params updating the audible sampler trigger payload.

Known 3A limitations:

- Manual binding UI is not implemented; current behavior is hint-driven.
- Rack target variants exist but scheduler/audio application is intentionally
  deferred.
- Cross-track writes and timebase writes remain out of scope.

#### Phase 3B — Mapping UI and binding status (next)

1. Extend target declaration parsing with mappable target ports:
   `(name :mappable)`, `(name :mappable target-kind)`, and
   `(name :mappable target-hint)`. Non-mappable `(name target-hint)` ports stay
   fixed/hint-following.
2. For `def-accumulator`, support fixed `:target target-hint`, unknown
   `:target :mappable`, optional `:target-kind`, and optional `:target-hint`.
3. When a selected step lane belongs to process slot X, show mapping widgets for
   all mappable target ports on X. Do not list every target under the process
   slot, and do not show map controls for fixed targets like transpose.
4. Add a port "map parameter" arm mode using the macro mapping wrapper seam:
   compatible params highlight, tag/name matches rank highest, click binds,
   unbind clears.
5. Persist manual choices into `TrackProcessSlot.bindings`, with explicit
   semantics for returning to hint-following vs. forced-unbound.
6. Show stale/unbound badges without changing runtime behavior: stale writes stay
   no-ops and other ports continue to apply.
7. Keep UI mapping same-track first. Cross-track and rack-slot application do not
   belong in this slice.

#### Phase 3C — Rack follow-up (deferred)

1. Implement actual rack-slot process-write application through the rack audio
   dispatch path.
2. Define `:self` on rack tracks in terms of the slot selected by the routed base
   event.
3. Add tests proving rack slot params and rack-slot instrument params are applied
   transiently and never mutate rack p-lock/default/snapshot storage.
4. Only after this should rack targets be presented as mappable/supported in the
   mapping UI.

### Phase 4 — Verdicts and ratchets

1. `(veto!)`: verdict in the run result; event marked dead; chain continues; state
   still advances. Dimmed-step preview for `:seed :locked` masks.
2. `(ratchet! :times :mode :span :shape)`: clones the base event
   (chord/p-locks/params intact), applies the `:shape` fold per sub-event,
   schedules via the pending-emissions store; sub-events routed through the
   existing `enqueue_due_process_emissions` / MIDI FX path. Ghost-step preview.

   **Two modes** (Cirklon's "repeat" has the same split). One verb, one clone
   path, one `:shape` fold — `:mode` only changes the onset/duration math when
   the scheduler materializes the clones, and reinterprets what `:span` means:

   | `:mode` | `:span` means | onset spacing | each hit's duration | total burst | feel |
   |---|---|---|---|---|---|
   | `:subdivide` (default) | total window to fill (default `step.duration`) | `span / times` | `span / times` | one step | classic roll / stutter |
   | `:repeat` | inter-onset interval (default `step.duration`) | `span` | base event's own duration (unchanged) | `times × span` | echo / delay-line, trails past the step |

   `:span` defaults to the step length in both modes, so the common case is just
   `(ratchet! :times n)`. Both modes are deterministic given
   `(times, mode, span)`, so ghost-step preview draws onsets at the computed
   offsets: `:subdivide` packs ghosts inside the step, `:repeat` shows trailing
   ghosts.

   **Overlap policy (open — see Open Questions):** in `:repeat` mode the burst
   extends past the step boundary, so trailing hits can collide with the next
   trig on the track. Default to **ring-through** (each clone is a normal
   downstream event with its own duration, per the "same downstream path" rule),
   leaving a `:choke`-at-boundary option for later. `:subdivide` never raises
   this.

3. **Process-inlet ports** (see "Process-inlet targets" above):
   `ParamTarget::ProcessInlet` and the connectable `:process-inlet` port mode
   are distinct from parameter-mappable ports; writes apply as transient
   overlays on the downstream slot's inlet resolution, composing with lanes per
   the op;
   instance-level wiring via `connect!` and the inline `:connect` selector form;
   soft no-op when no slot matches. Acceptance: dice → repeater `times`
   chain, including chain-reorder (write to an earlier slot lands next fire)
   and lane-composition (`set` vs `add`) tests.
4. `prob-mask`, `repeater`, and `dice` land as builtin library processes with
   presets. `repeater` exposes `mode` as a `:subdivide|:repeat` inlet.

### Phase 5 — Ordered-chain UI polish

Backend chain order is already meaningful: multiple process slots can be attached
to a track, and later slots see earlier transient writes. Remaining work is the
editing surface:

1. Reorder/remove in UI, with explicit order feedback.
2. Slot bypass/enable state in the chain editor.
3. Pattern-scoped settings/lane reconciliation polish across pattern switches.

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

## End Game (non-normative — doors to keep open)

The long vision, so earlier phases don't accidentally close doors on it. Not
scheduled; nothing below gates Phases 0–9.

The library should eventually hold **players, not just utilities**: a pack like
"es chord 2" loads from the browser, opens as a tab with its own panel (the
scripts-as-tabs mechanism already does this — the graph-neural scripts are the
precedent, including per-node track routing), lets the user say "observe these
2 melody tracks, play these 4 harmony tracks," and coordinates triggers into
shifting harmonies toward a panel-controlled goal density — while another
track's process lane sequences the pack's `chord` inlet.

Most of this already rides existing seams: custom tab UIs (scripts system),
track-typed inlets, cross-track **emissions** as the sanctioned cross-track
write (the conductor *plays* the harmony tracks, never edits their step data),
Phase 7 grabs/history for harmonic context, and `ProcessInlet` targets so
other processes/lanes sequence a conductor's inlets. Preset tier 3 becomes a
*style* ("close voicings, low density, follows the bass").

### Fields: suggestions instead of commands (the band model)

The richer version of coordination is not top-down. Alongside `:play` (emit
notes on bound tracks) and `:steer` (write followers' step params directly), a
process can **suggest**: publish a typed *field* — a pitch-set, a scalar
density, an accent gate — that any track may listen to on its own terms:

```lisp
;; conductor side — typed channel publish (domain rides with the field)
(suggest :harmony (pitch-field (chord-tones chord) :root chord :weight energy))
(suggest :density density)

;; band-member side — an ordinary chain process; acceptance is its knobs
(def-process follow-harmony
  :in ((listen :field :default :harmony)
       (amount :float 0 1 :default 1 :lane true)   ; obedience — sequenceable
       (grace  :int 0 3 :default 0))               ; allowed passing tones
  :run (let ((field (hear listen)))
         (when field
           (note! ev (draw-toward (note ev) field amount :passing grace)))))
```

Why this shape is right:

- **Acceptance is a process in the member's own chain.** Obedience is an
  inlet — lane-sequenceable, p-lockable, per-pattern. Interpretation is
  per-member (the pad snaps hard, the lead plays passing tones through the
  field). Refusal/counterpoint is a sign flip (`defy`).
- **Joining the band is one gesture.** The publisher never enumerates
  listeners; drop `follow-harmony` on a ninth track and it's in. This inverts
  the binding — `:play` roles bind conductor→track, fields bind
  track→field — and scales from quartet to orchestra unconfigured.
- **Decentralized, not dictatorial.** Any process can `suggest`, including
  band members — the drummer suggests `:accent` while following `:harmony`.
  Listening is a mesh, not a tree; "conductor" is a social role, not an
  architectural one. This maps to how people actually play together.
- **`hear` is nil-safe**: no publisher ⇒ follow processes are inert
  (defaults-inert / soft resolution, again).
- **Determinism**: `hear` reads fields as published at the *end of the
  previous tick* (same register rule as cross-track grabs) — one tick of
  latency, no ordering cycles, and reacting a beat late is musically human.
- **Collisions (decided): field names are plain channel names; two publishers
  on one field are the author's problem for now.** Blend policies / per-
  instance namespacing only if this bites in practice.
- Library shelf this implies: *band members* (`follow-harmony`,
  `follow-density`, `call-response`, `defy`) whose presets are personalities.

The command spectrum, in one line:
`:play` = the band's hands · `:steer` = lean on a player · `suggest` = the vibe.

The genuine engine deltas — the depth this spec doesn't yet cover:

1. **Conductor attachment mode.** One instance observing N tracks and playing
   M others — `(processes :observe (list 1 2) :play (list 3 4 5 6) ...)`-ish —
   invoked once per tick *after* all observed tracks resolve. This is where the
   currently-undefined same-tick cross-track ordering gets its answer: the
   determinism rule today says reads see end-of-previous-step registers; a
   conductor needs a defined view of "everything that fired this tick" across
   its observed set. Post-resolution invocation of a single instance is that
   answer without a general dependency graph. (Fields reduce how load-bearing
   this is — much of the band model works through opt-in listening with the
   one-tick rule, no track binding at all.)
2. **A determinism contract for stateful conductors.** A density-goal
   harmonizer with memory can't be a lane fold; it's the raw tier's "missing
   sugar shape" made concrete. Likely a third tier (`def-conductor`?):
   replayable because it is a pure function of (seeded RNG, resolved-state
   reads, inlets), not because it folds a lane. Lookahead/replay semantics for
   its emissions need the same rigor Phase 1 gives accumulators.
3. **Reactive outlet/channel → widget bindings.** Panels can *write* inlets
   today but can't cheaply *display* process state (current chord, achieved
   density). The graph engine's `bind-graph` reactive bindings are the
   precedent; processes need the analog. (Promotes the existing
   send/outlet-visualization open question from cheap-win to load-bearing.)
4. **Browser/packaging polish.** Scripts already deliver UI tabs; the gap is
   framing only — process packs as browsable library entries with presets,
   instrument-bank style.

Doors to keep open in earlier phases: keep the emission path fully
track-agnostic; keep `ProcessInlet` a first-class `ParamTarget` (it is the
seam for "sequence the conductor"); don't let chain-slot identity/storage
assume one-instance-per-track so hard that an N-track instance can't exist.

## Locked Decisions

- Immediate ordered write application (not final-merge); reads see pending writes.
- Veto does not halt the chain; state advances under masked trigs.
- Merge order: `patch default → key lock → step p-lock → process write → mods`.
- Process writes are transient fire-time overlays; never persisted into
  plock/step storage (protects `plock_variants` identity).
- `ParamTarget` data addresses rack slots from its first version (mirror the
  `plock_variants` domain split), but runtime rack write application remains
  deferred until the rack audio dispatch path applies transient overlays.
- Ports vs. bindings: defs declare named output ports. Non-mappable ports are
  fixed/hint-following; `:mappable` ports are user-assignable and may carry an
  optional compatibility kind or default selector. Hints (`step-param` /
  `param-tag` / `midi-fx-target`) are selectors, never addresses; concrete
  `ParamTarget` bindings are instance-level and track-identity-scoped. The
  arm-mode mapping UI shared with `MACRO_MAPPING_SPEC.md` is the next slice.
- Soft resolution: unbound or stale port writes are silent no-ops at fire
  time; the process still runs and state advances. Presets ship ports +
  hints, never addresses.
- Param tags are a small curated device-descriptor vocabulary (library-review
  gated), matched tag-first then exact-name; not a free-form ontology.
- Identity track-level, settings/lanes pattern-level; default reset on pattern change.
- Chain attachment implies the trigger (no `:on` for chain processes).
- Attachment authoring form: `(processes :track <n|:all|list> instance...)` —
  declarative whole-chain replace, class-call instances, `(lane ...)` literals
  on lane-backed inlets, returned handles + `lane!` for live tweaks.
- `(rand)` is the RNG verb; `:seed :locked | :per-cycle` on the instance.
- `MidiFxParam` targets early (Phase 3), timebase last (Phase 9).
- Ratchets clone the base event; sub-events go through the normal downstream path.
- Process-inlet ports: `(name :process-inlet)` declares a connectable port —
  defs never name another process class, and the port never appears in the
  parameter-mapping UI. Wiring is instance-level (`connect!` / inline `:connect`
  with `(process-inlet :class :inlet)` selectors), same-tick within the chain
  under immediate ordered application, transient overlay on the target slot's
  inlet resolution. Fan-out goes through channels, not ports.
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
- Initial param-tag vocabulary: exact list, and how existing instruments get
  retro-tagged (sweep the builtin/instrument descriptors once, or tag lazily
  as processes need them).
- Hint re-resolution UX for mappable ports with default hints: when a chain edit
  makes a *better* hint match appear while a port is already hint-bound
  elsewhere, does it re-bind (follow the hint) or stay put (stability)?
  Leaning: follow the hint until the user manually maps it, then never move.
- Whether per-pattern binding overrides are ever needed, or track-level
  bindings suffice (start track-level only).
- Ratchet `:repeat`-mode overlap: do trailing hits ring through the next trig
  (leaning default) or get choked at the step boundary? Add a `:choke` option
  only if ring-through bites in practice.
- Chain presets: process-inlet wires are chain-level state, so tier-1 presets
  can't capture a generator→consumer patch. Does a fourth preset granularity
  (whole chain: slots + bindings + optionally lanes) subsume tier 3, or sit
  beside it?
- Whether the `(process-inlet :class :inlet)` selector should also match by a
  future inlet *tag* (like param tags) instead of class name, for
  library-portable patches across e.g. different ratchet implementations.
- Preview of process-inlet-driven chains: Phase 8's curve preview must
  evaluate the chain prefix (producer folds feeding consumer folds) rather
  than single lanes — confirm this stays cheap enough for the lane UI.
