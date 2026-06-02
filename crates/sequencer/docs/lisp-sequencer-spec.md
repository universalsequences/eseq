# Lisp Sequencer Spec

## Goal

Generalize the sequencing model so that arbitrary sequencers — from a four-line chord sequencer to a Jaki-Liebezeit dot-dash rhythm engine to something as complex as the neural sequencer — can be authored **in lisp, in a single file, with their UI controls and visualizations declared inline next to the logic**. The neural sequencer (`neural.rs`, [neural-sequencer-spec.md](neural-sequencer-spec.md)) becomes the first *native* instance of a shared substrate; the lisp tier becomes the *open* instance of the same substrate. The two are deliberately kept as two faces of one system, not two separate features.

This is the "Max/Strudel" property the project was founded on — *"what if a patch editor like Max was done in Emacs, where lisp is the patch editor"* — applied to sequencers: throw a knob right inside the logic that drives it, and a visualization right next to the state it reflects.

## Motivation and framing

The eseq DAW already solves the pain that made a from-scratch patch editor overwhelming: tracks, instruments, effects, buses, and presets are unified first-class systems, so you no longer rebuild a DAW per project. What is *missing* is the flexibility to build a tiny bespoke sequencer (a chord cycler), or a complex generative one (neural), or an idiosyncratic one (Liebezeit cells), without dropping into Rust.

Three properties are non-negotiable, because they are exactly what the current Rust approach buys and we refuse to regress:

- **Speed.** The hot inner loop stays native. Lisp expresses *policy and intent*, not sample-rate math.
- **Serialization.** Sequencers serialize cleanly per-pattern. Code-is-data makes this nearly free.
- **Simplicity of timing.** Lisp never computes sample-accurate timing. It speaks musical/symbolic coordinates (timebases, beats, ticks); the engine owns samples, quantization, the lookahead queue, and `pattern_epoch` stamping.

## What already exists (grounding — verified against the code)

This spec builds on machinery that is already in the tree. Do not re-architect these; reuse them.

### Two coresident runtimes on the scheduler thread

- The **scheduler thread** (`"sequencer-scheduler"`, scheduler.rs:~2466) runs ahead of the audio RT thread, reads `Arc<SequencerSnapshot>` state, and pushes `ScheduledEvent`s into a lookahead queue (`ScheduledEventQueue<4096>`) that the cpal callback (audio.rs:~3737) drains. It advances `total_beats: f64` and detects per-grid boundary crossings.
- The **neural runtime is on the scheduler thread**, not the audio thread. It is a `&mut NeuralRuntime` owned by the scheduler loop (`neural_runtime` at scheduler.rs:~2480; `process_boundaries_with_outputs` called at scheduler.rs:~2434/3276; `process_seed_at` at ~3231/3262). The neural-sequencer-spec's "owned by the audio thread" wording is loose — it runs on the same non-RT thread as the lisp VM below.
- The **scheduler-thread lisp VM** is a persistent `ScratchControlRuntime` wrapping one `eseqlisp::Runtime` (lisp_effect.rs:~3153). It allocates freely (no RT constraint; lookahead absorbs jitter) and is **a separate instance from the UI VM** that renders panels (e.g. `neural-8x8-track-router.lisp`). Same interpreter, different builtin sets.

### A working reactive-transform lisp tier (accumulators + MIDI FX)

The scheduler-thread VM already runs user lisp per event:

- **MIDI FX** (`midi-fx/*/dsp.lisp`, invoked at lisp_effect.rs:~3433, chain runner scheduler.rs:~1968): per-track chains of 0..4 slots; input `MidiFxEvent` (resolved step, chord, step, track, note spans, params, `arp_phase_beats`), output `AccumulatorEvalOutput` (resolved, `suppressed`, `emitted: Vec`, modified params). Pre/post-accumulator position is a per-track param.
- **Script accumulators** (`def-accumulator`, invoked lisp_effect.rs:~3309): per-step transforms with persistent per-track value.
- **Emission in musical coordinates already exists:** `fx-emit` / `acc-emit` take a musical offset (timebase keyword like `:16`, or numeric source-step-relative), `:vel`, `:note`, etc., and the engine resolves to samples. Arp helpers (`fx-arp-emit`, `acc-arp-emit`) are duration-aware.
- **Persistent state already exists:** `fx-state-get` / `fx-state-set` over a `HashMap<String, EValue>` keyed per-track/per-FX (lisp_effect.rs:~3099/3181).
- **P-locks by identity already exist:** `ParamNodeId { logical_id, node_param_idx }` (neural.rs:30), with `acc-plock-effect`, `acc-set-instrument-param`, etc. baking values validated against param identity.
- **Runtime→UI telemetry already exists** as a hand-wired special case: `state.set_neural_visualization(...)` publishes a snapshot the UI reads via `SEQ.neural-energy-matrix`, `SEQ.neural-trigger-matrix`, `SEQ.neural-dampening-matrix`.

### The crucial gap

Accumulators and MIDI FX are **reactive transformers** — they only run when a step or keyboard event arrives. Neural is **generative** — neurons evaluate on their own `resolution` grid and can fire from accumulated energy with *no* incoming event this block. The lisp tier has no "evaluate me on my own timebase grid, every block, even with zero input." **That self-clock is the single genuinely missing primitive.** Everything else in this spec is reuse or thin wiring.

## Architecture: one substrate, two faces

```
                    ┌───────────────────────────────────────────┐
                    │  shared substrate (scheduler thread)        │
                    │  • generative clock  ← the new primitive    │
                    │  • emit (musical coords → samples)          │
                    │  • persistent state                         │
                    │  • plock baking by ParamNodeId              │
                    │  • velocity-merge + max_poly per track      │
                    │  • lookahead queue + pattern_epoch stamping │
                    └───────────────┬───────────────┬────────────┘
                                    │               │
                    ┌───────────────▼──┐     ┌──────▼─────────────────┐
                    │ NATIVE face       │     │ LISP face               │
                    │ edges/components/ │     │ def-sequencer:          │
                    │ propagation/      │     │ self-clocked lisp body  │
                    │ policy-menu       │     │ + param/state contract  │
                    │ (neural lives here)│     │ (chord seq, Liebezeit) │
                    └───────────────────┘     └────────────────────────┘
```

- The **native face** is the fast, constrained tier: a graph of stateful components on a shared clock, with a *menu* of behaviors (see "Activation × Distribution"). Neural is `(threshold × broadcast-weighted)`.
- The **lisp face** is the open tier: a self-clocked lisp body. A *policy hole* in the native graph may itself be a lisp callback, so the two faces share substrate rather than forking.

They must not drift into unrelated features. The same `emit`, the same clock boundary detection, the same plock baking, the same per-track merge serve both.

## The generative clock (keystone primitive)

The engine ticks a lisp closure on its declared timebase grid, reusing neural's exact cross-block boundary detection (`NeuralRuntime::next_eval_boundary` generalized to call a closure instead of evaluating a neuron — same handling of zero/one/several crossings per block, crossings spanning block edges, and the sample-then-index determinism contract).

The closure receives **musical position only**, never samples:

- `(gen-tick)` — integer count of this generator's grid boundaries since reset
- `(gen-beat)` — `f64` musical position of this boundary
- `(gen-bar)`, `(gen-phase)` — derived musical position
- transport reads (bpm, pattern, playing) via existing snapshot builtins
- a seeded `(gen-rand)` (reuse `splitmix64`; no wall-clock, no uncontrolled randomness — see Determinism)

It emits in musical units; the engine does every sample computation, the quantize snap, the queue push, and the epoch stamp:

```lisp
(seq-emit
  :track 2
  :at :now                 ; or (gen-offset :16 3) -> 3 sixteenths ahead, still musical
  :note 7                  ; transpose delta (existing note model)
  :vel 0.8
  :dur (beats :8)          ; duration in timebase units, not samples
  :chord (list 0 4 7)      ; optional
  :quantize :16            ; optional snap; engine resolves to a sample boundary
  :plock (list (plock-effect 0 2 0.5)        ; by slot+index, validated by ParamNodeId
               (plock-instr :cutoff 0.7)))   ; symbolic param name
```

Future-scheduling (delays, polyrhythm, multi-grid) falls out of `:at (gen-offset ...)` into the engine's existing queue — this is exactly neural's delayed-propagation mechanism reused. A generator declares a single base `:resolution`; finer/coarser behavior comes from decimating `(gen-tick)` in lisp or scheduling future offsets, so a single detector per generator suffices.

### Seeding (hybrid step + generative)

Reuse `process_seed`'s "route a step event to subscribers" idea: a generator may subscribe to a track, so a punched-in step both plays directly *and* seeds the generator. The matrix is additive — the generator only ever *adds* events, never delays or suppresses the seed hit (same parallel-mode decision as neural).

### Per-track merge applies to generator output

Generator-emitted events ride the same velocity-accumulation (same-sample coincidence → one accented hit) and `max_poly` voice cap as the rest of the engine, so coincident generator hits behave like neural's.

## Authoring model: `def-sequencer` in one file (UI VM)

The brutal limitation today is that authoring a sequencer means editing scheduler-VM scratch source, while its UI lives in a *separate* UI-VM file. The fix is **one authoring surface that targets both VMs** — not merging the VMs (the runtime split is correct), but letting the UI-VM file ship sequencer code to the scheduler VM.

### Mechanism: quote + remote-eval (enabled by same-interpreter)

`def-sequencer` is a **macro in the UI VM** that does not evaluate its body. Because both VMs are the same `eseqlisp` interpreter, code is just data:

```
UI VM:  (def-sequencer name (param ...) (state ...) :resolution :16 :tick (lambda () ...))
          │  macro: quote the :init/:tick/:resolution forms; extract param/state manifest
          ├──► manifest stays in UI VM as reactive state (drives controls + viz reads)
          └──► quoted body  ──channel──►  scheduler VM: eval → register generator
```

- Everything inside `:tick`/`:init` is **scheduler-VM code**; everything outside is **UI-VM code**. The macro enforces the split.
- **The only live data that crosses is the manifest.** Code crosses as quoted forms. **No closures over UI-VM heap inside `:tick`.** This single discipline is what keeps the system serializable and race-free.
- Helpers used by the body (e.g. `char-at`, math) must be in a **shared prelude** loaded into both VMs, or the body must be self-contained. The body may reference only: its params/state, scheduler builtins, and the shared prelude.

### Hot reload

Editing the file and re-evaluating re-ships the body. The scheduler VM swaps the definition by id/name **without nuking runtime state mid-bar** when structurally compatible, resetting otherwise — this is precisely the already-solved *"Preserve neural runtime state across network edits"* (commit `33caad3`). Same problem, same solution.

### "Compile" — interpret now, compile later

v1 ships the s-expr and **interprets** it on the scheduler VM (zero new compiler; per-tick interpretation cost absorbed by lookahead). If a hot generator later needs it, route *that body* through the existing dgenlisp→C path — only the bodies that earn it. Keep the sequencer-logic dialect (plain eseqlisp control flow) distinct from dgenlisp (compiled sample DSP); param *expressions* are the natural dgenlisp candidate, not whole tick bodies.

## The data contract: `param` and `state`

The contract is deliberately one-directional per variable (the bidirectional `:inout` case is **out of scope** — see Out of Scope). Two declarations, symmetric:

| | `param` | `state` |
|---|---|---|
| direction | UI → runtime | runtime → (UI and/or internal) |
| authoritative writer | UI | runtime |
| serialized | **yes** (authored) | no (transient; `:persist` survives hot-reload, resets on transport/pattern reset) |
| update cadence | event-rate (on change) | block-rate, latest-wins |
| in UI | drives a control (`on-change` writes) | read-only viz (`:visible`) |
| in `:tick` | read by name | read/write by name |

### `param` (UI → runtime, authored, serialized)

One declaration is simultaneously: a reactive UI binding, a typed snapshot slot the scheduler reads, and a serialization unit.

```lisp
(param density :type :float :min 0 :max 1 :default 1 :ui :knob)
(param cell    :type :string :default ".. .- .  ..- . .-" :ui :text)
(param track   :type :track  :default 1)
```

Types: `:float`, `:int`, `:enum` (with `:options`), `:string`, `:track`, `:timebase`, `:vector` (`:len`), `:matrix` (`:rows`/`:cols`). UI hints: `:knob`, `:slider`, `:dropdown`, `:text`, `:matrix`. Inside `:tick`, a param is in scope by name. From the UI, `(seq-param name density)` is a reactive read and `(seq-set! name density v)` writes (generalizes `seq-set-midi-fx-param`).

### `state` (runtime-owned; `:persist`? × `:visible`?)

"The inverse of param." Two orthogonal flags give a complete 2×2:

| | not `:visible` | `:visible` |
|---|---|---|
| transient | scratch counter | instantaneous viz (fire LED) |
| `:persist` | internal energy you don't show | neuron energy vector (persists *and* watched) |

```lisp
(state phase  :type :int)                                       ; internal
(state energy :type :vector :len 16 :persist true :visible true) ; both
(state fire   :type :vector :len 16 :visible true :hold :8)      ; viz only
```

Written in `:tick` via `(state-set! name ...)` / `(state-get name)`; read in the UI via `(seq-state name var)` reactive binding (generalizes `SEQ.neural-energy-matrix`).

### Telemetry transport semantics (what makes `state` *not* a param)

- **Latest-wins register, never a queue.** Writes during `:tick` go to a runtime-owned working copy; **once per block** (not per tick) the scheduler publishes it via Arc-swap (exactly like `set_neural_visualization`). The UI samples the latest at frame rate. Dropping intermediate values is *correct* — the opposite of the event channel. Cross-thread traffic stays at block rate regardless of tick rate.
- **Runtime is authoritative, UI is read-only** for `state`. No write-back.
- **Fixed shape** (declared `:len`/`:rows`/`:cols`) keeps the lock-free double/triple buffer trivial. Lean fixed-shape.

### Declarative visual smoothing

Tick-rate is faster than frame-rate, so an instantaneous pulse is invisible (neural solved this with `trigger_visual_until_beats` + `TRIGGER_VISUAL_HOLD_BEATS`). Make it declarative on visible state instead of hand-written per sequencer:

- `:hold :8` — hold the last nonzero value for an eighth-note
- `:decay 0.9` — exponential bleed per block

The engine applies smoothing on the publish path.

### Two telemetry *shapes*

- **register** (latest value: energy, LEDs, current step) — covers most cases.
- **ring** (bounded history: the spec's bottom "output timeline of generated voices over bars"):

```lisp
(state recent :type :ring :len 256 :visible true)   ; append-only history the UI scrolls
```

The engine may auto-populate a ring from `seq-emit` to give a free event-timeline viz. Register = "what is true now"; ring = "what happened recently."

## Full one-file example (Jaki Liebezeit dot-dash)

```lisp
(def-sequencer liebezeit
  ;; --- contract: declared once, drives UI controls AND feeds the runtime ---
  (param cell    :type :string :default ".. .- .  ..- . .-" :ui :text)
  (param density :type :float  :min 0 :max 1 :default 1 :ui :knob)
  (param track   :type :track  :default 1)
  (state fire    :type :vector :len 16 :visible true :hold :8)   ; runtime -> UI viz

  ;; --- this body is QUOTED and shipped to the scheduler VM ---
  :resolution :16
  :tick (lambda ()
    (let ((c (char-at cell (mod (gen-tick) (len cell)))))
      (if (and (= c ".") (< (gen-rand) density))
        (do
          (state-set! fire (mod (gen-tick) 16) 1)
          (seq-emit :track track :at :now :dur (beats :16) :vel 0.95))))))

;; --- this runs in the UI VM, same file, knobs next to the logic ---
(effect-buffer "*liebezeit*"
  (v-stack :gap 0.5
    (text-input :value (seq-param liebezeit cell)    :on-change (lambda (v) (seq-set! liebezeit cell v)))
    (knob       :value (seq-param liebezeit density)  :on-change (lambda (v) (seq-set! liebezeit density v)))
    (matrix     :value (seq-state liebezeit fire))))
```

### Other shapes the model must cover

- **Chord sequencer** (`:resolution :1`, read a table, `seq-emit :chord` per bar).
- **Neural-in-lisp** (the proof of generality, not the shipping path): energy `state` vector + weight-table param, each tick add propagation / threshold / emit / decay. If this expresses, the substrate spans the full range. Ship neural *native* for speed; keep this as the conformance test that the lisp face is general enough.

## Activation × Distribution (the native face's constraint)

The native graph's behavior menu is the product of two small orthogonal axes — this is both the unification of neural and Markov and the productive constraint that keeps the open-ended space tractable:

- **Activation** (when a node fires): `threshold(energy ≥ θ)` · `always(if-active)` · `probability(p)` · `counter(euclidean)`
- **Distribution** (how a fired node feeds targets): `broadcast-weighted` · `select-one-stochastic` · `select-one-roundrobin`

Neural = `(threshold × broadcast-weighted)`. Markov = `(always × select-one-stochastic)`. Same graph, same clock, same emit, same seeding — only the (activation × distribution) pair differs. A second sequencer (Markov) dropping in as config rather than special-casing is the validation that the substrate is real.

## Determinism contract

Generators (native and lisp) must be pure functions of (transport position, config/params, persistent state):

- No wall-clock. Randomness only via seeded `(gen-rand)`.
- Multiple generators crossing the same boundary order by **sample-time first, then generator index** — inherited free *iff* the clock reuses neural's loop rather than reinventing it. Make that reuse a hard requirement.
- A generator whose boundary lands at a block edge must resolve identically whether it falls at the end of one block or the start of the next (the neural cross-block test).

## Cost and safety

- Per-fine-grid lisp across several generators on the scheduler thread; lookahead absorbs jitter but **not sustained overload**. The native menu (neural et al.) is the pressure valve for heavy work.
- Provide a per-generator CPU budget / log so a runaway generator degrades audibly-gracefully (drops to a logged safe state) rather than silently starving the lookahead horizon.
- Telemetry publish is block-rate and lossy by design — it cannot back-pressure the tick.

## Implementation Phases

Ordered so each is independently shippable and the pipeline is never broken.

### Phase 1: Generative clock primitive (native)
- Generalize `NeuralRuntime::next_eval_boundary` / boundary detection into a reusable per-generator clock that invokes a callback, preserving the sample-then-index determinism contract and cross-block behavior.
- Prove parity by re-expressing neural through it (neural's existing test suite is the golden oracle).

### Phase 2: `seq-emit` from a self-clocked context
- Extend the existing `acc-emit`/`fx-emit` emission (musical offset → sample, quantize snap, plock baking, queue push, `pattern_epoch` stamp) to be callable from a generative tick, not only a reactive event.
- Seeding: route step events to subscribed generators (`process_seed` reuse). Velocity-merge + `max_poly` apply to generator output.

### Phase 3: `def-sequencer` authoring + code-shipping
- `def-sequencer` macro in the UI VM: quote `:init`/`:tick`/`:resolution`, extract the param/state manifest, ship the quoted body over a channel to the scheduler VM, register a generator.
- Shared prelude loaded into both VMs. Enforce the crossing rule (manifest as data, body as quoted forms, no closures over UI heap).
- Hot-reload swap-by-id preserving compatible runtime state (mirror commit `33caad3`).

### Phase 4: The data contract
- `param`: reactive UI binding + typed snapshot slot + serialization unit; `(seq-param ...)` / `(seq-set! ...)`. Types and UI hints above.
- `state`: runtime-owned, `:persist` × `:visible`; `(state-get/set!)` in tick, `(seq-state ...)` reactive read in UI.
- Telemetry transport: block-rate latest-wins Arc-swap (generalize `set_neural_visualization`); fixed-shape buffers; `:hold`/`:decay` smoothing; `:ring` history shape.

### Phase 5: Native behavior menu + Markov validation
- Implement the activation × distribution menu on the generative-clock substrate.
- Add Markov as `(always × select-one-stochastic)` — the proof the abstraction is real (drops in as config, no special-casing).
- Optional: expose a policy hole that accepts a lisp callback, unifying the two faces.

## Out of Scope, Logged for Future

- **Bidirectional `:inout` variables** (a var both UI-scrubbable and runtime-advanced). Two authoritative writers across the VM boundary add disproportionate race/policy machinery. The "grab the playhead while running" case is composed instead from a `param` (scrub target, UI-authoritative) + a `state` (position, runtime-authoritative) reconciled in `:tick` — keeping authority one-directional per variable.
- **Compiling whole tick bodies to native** (beyond interpreted remote-eval). Revisit only for hot generators, reusing the dgenlisp→C path, and only for param expressions first.
- **Dynamic param manifests** (a generator declaring params at runtime based on its own logic). Lean static + structured-typed (scalar/vector/matrix) manifests; revisit dynamic if a real need appears.
- **Merging the UI and scheduler VMs.** The runtime split is correct; only the authoring surface is unified (code-shipping).
- **Project-persisting transient runtime state** across loads (resume mid-energy). `:persist` survives hot-reload but resets on transport/pattern reset; full project-resume is future work.
- Learned/evolving weights, cross-pattern morphing, audio-rate propagation — inherited from [neural-sequencer-spec.md](neural-sequencer-spec.md) Out of Scope.
