# Processes and Channels Spec

First-class musical processes for eseqlisp: Max/MSP-style objects (inlets, outlets, state, clock) expressed as lisp instead of patching, with Strudel/Tidal-flavored affordances layered on top as library.

## Goal

Make "a thing that runs against the clock, listens to musical state, and mutates the engine" a first-class, composable, live-codable concept. Motivating uses:

- a process that adapts neural-sequencer edge weights from observed firing activity (Hebbian-style)
- a process that walks a global transpose by brownian motion, quantized to the beat
- generators (LFOs, ramps, pattern steppers) whose values fan out to any mutation surface: graph params, effect params, mixer, future signal-rate modulation inputs
- eventually, the same nesting/channel conventions carrying into signal-rate objects: `(mc.cycle~ (mc.line~ control-channel))`

## Non-Goals

- No new evaluation model. Process bodies are plain eseqlisp evaluated by the scheduler thread (see `control-thread-sequencer-spec.md`).
- No coroutines or user-visible threads. Time lives in declarative clauses (`:every`, `:listen`), so the scheduler stays in charge and lookahead quantization is free.
- No replacement for the reactive UI binding system. Channels and reactive bindings stay distinct (musical-time flow vs. render-time binding); they bridge at exactly one point (`bind-chan`).
- No RT-thread lisp. Processes never run on the audio callback.

## Design principle

**Dramatic simplicity, power from composition.** The kernel is three concepts and two conventions; everything musical is library built from them. If the kernel ever grows a sibling (a second kind of channel, a second messaging convention, a special process type), that is the smell to stop and reconsider.

---

## Kernel concept 1: Process

An object with named **inlets**, **outlets**, **state**, and a **clock**. `def-process` defines a class; calling the class instantiates it and returns a handle.

```lisp
(def-process brownian
  :in    ((step  :float 0 12 :default 1)     ; max wobble per tick
          (range :float 0 48 :default 12)    ; reflecting bounds ±range
          (pull  :float 0 1  :default 0.02)) ; mean-reversion toward 0
  :out   ((value :float))
  :state ((x 0))
  :every (beats 1)
  :run (do
         (set! x (reflect (- (in :range)) (in :range)
                   (+ (* x (- 1 (in :pull)))
                      (rand (- (in :step)) (in :step)))))
         (out :value x)))

(def wander (brownian :step 0.7 :range 7))
(start wander)
(stop wander)
```

Clauses:

| Clause | Meaning |
|---|---|
| `:in` | Inlet declarations, `def-node`-param style: `(name type [min max] :default v)`. An inlet may hold a constant, a channel, or another process's outlet (a patch). `(in :name)` reads the resolved current value inside the body. |
| `:out` | Outlet declarations. `(out :name v)` publishes. An outlet is readable as `(handle :name)` in patch position. |
| `:state` | Named state cells, like `def-node` `:state`. Survive hot-swap by name (see Conventions). |
| `:every` | The built-in metro: run `:run` at this musical interval, quantized (see Quantization). Accepts musical-time values: `(beats n)`, `(bars n)`, `:16`, etc. May reference an inlet: `:every (in :rate)`. |
| `:listen` | Event inlets: named subscriptions to event sources (e.g. `(fires (seq-fires (in :seq)))`). |
| `:on-<name>` | Handler for a `:listen` inlet, run per event with the event bound: `:on-fires (|e| ...)`. |
| `:run` | Body evaluated on each clock tick. |

Anonymous processes: the clock-sugar forms (`every`, `after`, ...) are process instances with only a clock and a body — no `def-process` required for throwaway ideas:

```lisp
(defstate drift 0)
(every (beats 1)
  (do
    (set! drift (clamp -12 12 (+ drift (rand -1 1))))
    (transpose! (round drift))))
```

They return handles too, so they can be named, stopped, and messaged like any instance.

## Kernel concept 2: Channel

A named stream — the send/receive pair. Two flavors:

- **value channel**: holds its last value; readers can sample it at any time. `(defchan wobble 0.0)`
- **message channel**: bang/message stream, no held value. `(defchan retrig)`

```lisp
(send wobble 0.4)
(tap wobble |v| (transpose! (round v)))
```

Channels are **late-bound by name**. A `tap` or `patch` referencing a channel that is redefined keeps working; re-evaluating a buffer never breaks wiring. This is what makes live re-eval safe.

## Kernel concept 3: Patch

A connection from an outlet to an inlet or channel. Two forms:

- **Implicit (nesting)**: a process expression appearing in inlet position is a patch cord. Same convention dgenlisp already uses for signal graphs.
- **Explicit**: `(patch (wander :value) wobble)` for non-nested / fan-out wiring.

`tap` is the terminal patch: outlet-or-channel into an arbitrary lisp lambda, which adapts the value onto whatever mutation surface it likes.

```lisp
(defchan drift 0.0)
(patch ((brownian :step 0.7) :value) drift)

(tap drift |v| (transpose! (round v)))                     ; whole engine
(tap drift |v| (graph-param "neural-8x8" 0 :threshold     ; and/or one node
                 (+ 0.55 (* 0.02 v))))
```

Objects generate values, channels distribute them, taps adapt them to the mutation surface. Generators never know their destinations.

---

## Convention 1: handle + keyword = message to inlet

```lisp
(wander :step 3)      ; get twitchier, live
(wander :pull 0.2)    ; yank back toward center
```

One calling convention covers construction-time configuration, live tweaking from the scratch buffer, and inter-process messaging. It is the Max "send a message into an inlet" gesture without the patch cord.

## Convention 2: hot-swap preserves state by name

Re-evaluating a `def-process` rebinds behavior on all running instances of that class **without resetting them**: `:state` cells are carried over by name (new cells get their initializer; removed cells are dropped). Follows the pattern established in `lisp-hot-reload-spec.md`. This is the single biggest determinant of "live coding" vs. "restart the patch."

---

## Timing semantics

### Execution home

Processes run on the scheduler thread from `control-thread-sequencer-spec.md`. Bodies are evaluated at lookahead time against a coherent snapshot; outputs that are musical events are timestamped into the RT event queue exactly like `acc-emit`. The audio thread never runs process lisp.

### Quantization (the `:every` rule)

`:every (beats 1)` means the effect **lands on the beat, sample-accurately**: the scheduler evaluates the body ahead of the transport and timestamps its outputs to the boundary. Quantized-and-timestamped is the default; `:phase 0.5` (fraction of the interval) runs deliberately off-grid. There is no free-running metro that drifts against the transport.

### Engine mutations from process bodies

Calls like `graph-param` / `transpose!` inside `:run` are applied at the tick's boundary timestamp, not at evaluation time. (Implementation: the mutation surface, when invoked in scheduler context, enqueues a timestamped state change rather than writing immediately.)

### Global transpose semantics

`transpose!` (new engine setter, to be added alongside the existing mutation surface) affects **future note-ons only** — no pitch-bend of sounding voices. Implementation is a transpose offset applied in the scheduler layer at event-emission time. Per-track opt-out is required (drums should not wander).

---

## Library (no new semantics — all composition)

### Clock and event sugar

```lisp
(every (bars 4) body...)          ; anonymous quantized process
(after (bars 16) body...)         ; one-shot
(sometimes 0.25 body...)          ; probabilistic wrapper
(on (track-fires 3) |e| body...)  ; anonymous process with only a :listen
```

`on` is the event-flavored sibling of `every`: an anonymous process instance with a single `:listen` inlet and its handler. Returns a handle like everything else.

### Event emission: `emit`

The process-world generalization of `acc-emit` (see `lisp-midi-fx.md`): inside a `:listen` handler (or `:run` body), schedule a derived event at a musical offset. Inherits all fields from the source event unless overridden; offsets are relative to the **source event's timestamp**, not evaluation time, so results are sample-accurate.

```lisp
(on (track-fires 3) |e|
  (when (> (get e :vel) 0.6)
    (emit e :track 5 :after (beats 1))))    ; conditional echo, 1/4 note later
```

`play-pat` composes `emit` with a pattern value — the whole burst is determined and timestamped the moment it is called:

```lisp
(play-pat (pat "0 0 [4 12]*6 ~ ~ 0")
  :from e            ; inherit vel/dur; degrees are semitone offsets from e's note
  :track 5
  :after (beats 1)
  :over (bars 1))    ; cycle length; defaults to 1 bar (Tidal cycle)
```

Because `pat` values are data, a reusable process can take the pattern as an inlet and have it live-swapped via the handle-message convention.

**Pending-emissions store**: emissions may land past the lookahead horizon (e.g. a bar-long burst), so `emit` output goes into a scheduler-side pending store keyed in beats — the same representation as `EmittedEvent` for MIDI FX — and is converted to sample time as the horizon reaches it. Pending emissions are **cancelled on pattern/scene change** (a burst from the old scene should not bleed into the new one) and flushed on transport stop/relocate.

### Generators (processes with outlets, no side effects)

```lisp
(lfo :rate (bars 8) :shape :tri)
(brownian :step 0.7 :range 7)
(line! ch :to 0.8 :over (bars 2))   ; musical-time ramp into a channel
(euclid 5 16)                        ; pattern-valued
```

### `pat` mini-notation

Tidal-style mini-notation as a pattern **value** (data type + parser, not an execution concept). Cycles per bar; clock processes step through it:

```lisp
(pat "0 3 [5 7] ~")
(every (bars 1) (step-through (pat "0 ~ 3 [5 7]") |n| ...))
```

Priority: implement a minimal `pat` early — sequence, subdivision `[..]`, rest `~`, repetition `*` — because the mini-notation is most of the live-coding joy; extensions (`<>` alternation, `?` degrade, `,` polyphony) come later.

### Event sources for `:listen`

`seq-fires` (graph-sequencer node firings) is the first source, but only one among many. Candidates: step triggers per track, transport events (pattern change, bar boundaries), channel messages, MIDI input.

### Introspection

- `(ps)` — list running process instances with class, clock, and current inlet values.
- `bind-chan` — expose a channel as a reactive binding so any effect-buffer widget can display or write it. This is the single bridge point between channels and the reactive UI system.

---

## Worked examples

### Hebbian weight adaptation (the motivating case)

```lisp
(def-process hebbian
  :in    ((seq   :sequencer)
          (rate  :float 0 1 :default 0.05)
          (decay :float 0 1 :default 0.98))
  :state ((co (zeros 8 8)))
  :listen ((fires (seq-fires (in :seq))))
  :on-fires (|e|
    (mat-decay! co 0.999)
    (co-activate! co (get e :node)))
  :every (bars 1)
  :run
    (for-each-edge (in :seq) |r c w|
      (edge! (in :seq) r c :weight
        (clamp -1 1 (+ (* w (in :decay))
                       (* (in :rate) (mat-get co r c)))))))

(def h (hebbian :seq "neural-8x8-reset-demo" :rate 0.08))
(start h)
(h :rate 0.2)
```

### Brownian global transpose

Level 1 (throwaway) shown under Kernel concept 1. Level 2/3 (reusable object → channel fan-out) shown under Kernel concept 3. All three levels use the same primitives; naming and instantiating a class is never required to try an idea.

### Conditional echo / pattern burst (event-level composition)

Throwaway forms shown under "Event emission" above. The reusable object — note the pattern is just an inlet value, so `(shadow :pattern (pat "0 [3 3] 12 ~"))` reshapes the burst live:

```lisp
(def-process burst-echo
  :in ((from :track) (to :track)
       (min-vel  :float 0 1 :default 0.6)
       (delay    :beats :default (beats 1))
       (pattern  :pat   :default (pat "0"))
       (span     :beats :default (bars 1)))
  :listen ((trig (track-fires (in :from))))
  :on-trig (|e|
    (when (> (get e :vel) (in :min-vel))
      (play-pat (in :pattern)
        :from e :track (in :to)
        :after (in :delay) :over (in :span)))))

(def shadow (burst-echo :from 3 :to 5 :pattern (pat "0 0 [4 12]*6 ~ ~ 0")))
(start shadow)
```

### Future signal-rate reach

The nesting convention is rate-agnostic. A `chan` appearing in a signal-position inlet compiles to a modulation input (the dgenlisp modulation-manifest seam), so control processes write channels and audio graphs read them at signal rate:

```lisp
(mc.cycle~ (mc.line~ control-channel))
```

needs zero new syntax when that layer arrives.

---

## Runtime boundary: authoring VM vs scheduler VM

There are two ESeqLisp runtimes: the UI runtime (regular buffers, where authoring happens) and the scratch/control runtime (where the scheduler executes process bodies). Lisp closure values are **not portable** between them — compiled function IDs, environment cells, and host handles are VM-local. The boundary rule:

**Process definitions cross the boundary as source text, in exactly one place.** `def-process` bodies ship as source and are compiled in the scheduler VM. Everything callback-shaped lowers to that path — there is no separate callback-publication mechanism:

- `every`, `after`, `on`, and `tap` are all sugar for an anonymous process instance. In particular, `tap` lowers to a process with a single `:listen` inlet on the channel and the lambda as its handler. Taps thereby inherit process shipping, hot-swap, `(ps)` visibility, `start`/`stop`, and error reporting for free.

### Capture rules (apply to ALL shipped bodies)

At publish time (in the UI runtime), free variables of a shipped body are resolved and inlined **by value**. Portable values: numbers, strings, keywords, bools, nil, lists/maps of those, `pat` values, channel references (by name), process handles (by name). Anything else — functions, widgets, host handles, `Rc` values — is an immediate authoring-time error naming the variable and suggesting the fix ("reference a channel or make it a process inlet").

Capture is a snapshot; it does not track later mutation of the source binding. Live-tunable values belong in channels or process inlets — that is the sanctioned mutable capture.

### Identity and ownership

A shipped declaration's ID defaults to (buffer name, ordinal within that buffer's evaluation), with optional explicit `:key` for stability across reordering — the same convention as UI widget keys. Re-evaluating a buffer republishes the buffer's full declaration set; the scheduler diffs by ID: changed declarations are replaced (preserving `:state` by name per the hot-swap rule), absent ones are stopped and removed. The buffer is the ownership unit, exactly like effect-buffer widgets.

### Scheduler-safe native set

Shipped bodies may call: pure lisp, the mutation surface (`graph-*`, `transpose!`, plocks), and process natives (`emit`, `send`, `play-pat`, handle messages). They may not call UI natives (`reactive-set`, widgets) or buffer/host natives. Call sites are checked against the whitelist at publish time in the UI runtime so violations surface as immediate buffer diagnostics; the scheduler VM keeps a backstop check.

### The reverse direction (scheduler → UI)

UI reactions to channel activity do not cross into the scheduler. Channel last-values/messages are mirrored back through the `SequencerState` snapshot; `bind-chan` reads them reactively, and an optional `tap-ui` runs a callback locally in the UI runtime (frame-rate timing, no mutation surface). Rule of thumb: `tap` = musical time, scheduler VM, sample-accurate; `tap-ui`/`bind-chan` = render time, UI VM.

---

## Deliberately excluded

- Second wiring mechanism, second channel kind beyond value/message, special-cased process types.
- Free-running (transport-independent) clocks.
- Process-to-process synchronous calls; all inter-process communication is inlet messages or channels.
- Any coupling to the graph sequencer beyond `seq-fires` being one event source.

## Persistence

Solved by the existing script mechanism: processes are code, and code lives in scripts (`crates/sequencer/scripts/`) pulled into a project via the script selector (`ctrl-c s`), which adds a `(load "...")` line to the scratch buffer. The scratch buffer is persisted and re-loaded with the project, so any `def-process` / `defchan` / `start` calls in it come back with the project. No new serialization is needed — a process definition is no different from a `def-sequencer`.

One convention carries over from the graph-sequencer scripts: loading a file should publish definitions but not necessarily start instances with side effects; scripts that auto-`start` mutating processes on load should make that explicit and obvious (or gate it behind a `script-init-fn`-style entrypoint, matching the existing "loading does not write overrides" convention).

## Open questions

- Error policy: a `:run` body that throws should stop that instance and surface in the UI, never stall the scheduler tick.
- `:listen` backpressure: event handlers run at event timestamps within lookahead; a slow handler must not delay unrelated processes — budget/deadline per tick TBD.
- Fan-in on inlets: last-writer-wins on value inlets (Max-like), or explicit combiner? Leaning last-writer-wins.
