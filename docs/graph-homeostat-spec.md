# Graph Homeostat: Process-Driven Delta Regulation for Neural Sequencers

Status: draft spec, 2026-07-20. Companion to
`docs/cirklon-process-accumulator-brainstorm.md` (process engine, normative),
`docs/cirklon-endgame-trajectory.md` (landed-slice inventory), and
`crates/sequencer/scripts/sequencers/graph-neural-variable-reset-demo.lisp`
(the graph sequencer this regulates).

## 1. Motivation

The graph-mode neural sequencers are deterministic energy machines: seeds,
weights, delays, transposes, thresholds. They are rich to *author* but static
to *listen to* — the built-in randomness knobs add noise, not behavior.

This spec closes the loop with the process layer: a **homeostat process**
observes what the graph is actually playing (via the landed Phase 7 read
family) and applies corrective nudges to weights / node transposes / delays so
the output stays inside an authored **viability envelope** — e.g. "between 3
and 9 fires per bar, pitch spread at least an octave." Mutations become
purposeful responses to the music instead of dice rolls.

The framing is Stafford Beer's Viable System Model, and the mapping is
structural, not decorative:

| VSM level | eseq realization |
|---|---|
| System 1 — operations | The graph sequencer itself: autonomous, energy-driven. |
| System 2 — coordination | Per-track process chains (already landed). |
| System 3 — here-and-now regulation | The **homeostat**: measures the audible output each bar, nudges parameters toward the viability envelope. |
| System 4 — outside-and-future | A slower **restructurer**: watches whether System 3 is coping; when the nudge controller saturates, it intervenes structurally (rotate seeds, reshape topology). |

Two design principles carried throughout:

1. **Observe behavior, not internals.** The regulator reads the routed
   *tracks* (resolved values, trig histories), not the graph's internal
   energy state. Steering by observed output is the cybernetically honest
   loop and requires almost no new read surface.
2. **Never write the authored layer.** All regulation lands in an ephemeral
   delta overlay (§3). The user's saved patch — knob values, scene captures,
   pattern overrides — is untouchable by processes.

## 2. The problem this solves

Today a process *can* mutate the graph: `:run` bodies evaluate in the same
lisp runtime where `graph-node` / `graph-param` / `graph-edge` /
`graph-config` are registered (`lisp_host.rs`, `invoke_process_run`). But
those setters write **`ProjectGraphOverrides`** — the per-pattern persisted
layer that the UI knobs edit, scenes capture, and undo tracks
(`graph.rs::runtime_config_with_overrides`). A regulator writing there would:

- clobber the authored start state (stop + restart does not restore the
  patch the user saved),
- spam the undo history and override store at musical rates,
- fight the user for the same knobs while both are editing.

So process-driven regulation gets its own layer.

## 3. The delta overlay

A third resolution layer, above the persisted overrides, holding **additive
deltas** keyed by target:

```
effective = clamp(manifest-default ⊕ authored-override + delta,
                  declared param range)
```

(`⊕` is the existing override-or-inherit resolution; the delta is a plain
float addend applied after it.)

Delta keys mirror the override surface the homeostat needs:

| Key shape | Examples |
|---|---|
| `(node, intrinsic-field)` | `:delay` |
| `(node, param-field)` | `:transpose`, `:threshold`, `:vel-decay` |
| `(from, to, edge-field)` | `:weight`, `:dampening` |

Config-level fields (`:node-count`, `:max-poly`, `:reset-every`) are **not**
delta-able in v1. Structural change is System 4's job and even there it works
through node-level deltas (§7); changing node count live is a rebuild, not a
nudge.

### 3.1 Storage and typing

- Deltas are stored as `f32`, including for int-typed fields (`:delay`,
  `:transpose`). Quantization (round toward the authored value) happens at
  resolution time only, so leak and small nudges accumulate smoothly instead
  of being eaten by rounding.
- The store lives with the scheduler-owned live graph runtime (next to the
  energy accumulators), **not** in `ProjectGraphOverrides` and not in any
  serialized project state. It is never persisted.
- Sparse map; absent key = delta 0. A delta whose magnitude leaks below
  epsilon (1e-4) is removed.

### 3.2 Lifecycle

| Event | Deltas |
|---|---|
| Transport **stop** | **cleared** — restart reproduces the authored patch exactly |
| Pattern change / scene change | **cleared** (conservative default; see Open Questions) |
| Graph periodic `:reset-every` boundary | **kept** — this is the homeostat's accumulated regulation; clearing it every phrase would lobotomize the controller |
| Node-count grow/shrink | kept for surviving keys, dropped for out-of-range keys (same policy as dormant overrides, except deltas are simply dropped, never parked) |
| Explicit `graph-clear-deltas!` | cleared |
| Commit (§5) | folded into authored overrides, then cleared |

### 3.3 Leak

Deltas decay multiplicatively toward zero so a quiet (or dead, or buggy)
regulator lets the graph drift home to the authored patch — the most
Beer-like failure mode and a hard cap on how far anything can wander.

- One leak coefficient per graph instance, expressed as **per-beat factor**,
  applied on the graph scheduler's step boundary (scaled by step size, same
  pattern as `:energy-decay`'s `per-step` scaling).
- Default: `0.9946` per beat ≈ half-life of 128 beats (32 bars at 4/4).
  Slow enough that regulation accumulates across phrases, fast enough that
  an abandoned session audibly returns home.
- Settable per graph: `(graph-delta-leak! name factor)`; `1.0` disables leak.
- Leak operates on the stored float; int-field quantization stays at
  resolution time (§3.1).

## 4. New natives

### 4.1 Process-side writes (delta layer only)

Mirror the existing setter shapes, suffixed `-nudge!`, taking a **delta**:

```lisp
(graph-nudge-param! name node :transpose 1.5)      ; node param delta
(graph-nudge-node!  name node :delay -0.5)         ; node intrinsic delta
(graph-nudge-edge!  name :from f :to t :weight 0.05)
```

Semantics:

- Adds to the existing delta for that key (nudges compose across invocations).
- The *effective* value is clamped to the param's declared range at
  resolution time; the stored delta is additionally clamped so that
  `|delta| ≤ range-width` (prevents unbounded invisible windup — the
  classic integrator-windup guard).
- Callable from process `:run` bodies. Under the hood these do **not** touch
  the runtime directly: they append a `GraphNudge` command to the
  invocation's `commands` vec (`ProcessRunResult.commands`, the sanctioned
  side-effect channel), which the scheduler applies at the same point it
  applies other process commands. Calling them outside a process invocation
  (e.g. from the UI scratch buffer) applies immediately via the host — handy
  for testing.
- Best-effort determinism tier, same as raw `def-process` today.
- Rate expectation: musical rates (per bar, per phrase). The command path
  makes per-tick nudging *safe*, but the leak/windup design assumes coarse
  corrections; document, don't enforce.

### 4.2 Reads

```lisp
(graph-delta name node :transpose)                 ; current delta (0 if none)
(graph-delta-edge name :from f :to t :weight)
(graph-effective-param name node :transpose)       ; authored ⊕ override + delta, clamped
```

The existing `graph-node-value` / `graph-param-value` / `graph-edge-value`
reads keep their current meaning (authored layer) — UI sync code depends on
that.

### 4.3 One read-family addition: fire counting

The Phase 7 read family returns param *values* (`:steps-ago` / `:trigs-ago`);
density measurement needs to count fires in a window. The engine already
keeps 256 fired-trigger histories per track, so this is exposure, not new
bookkeeping:

```lisp
(read (track n :fire-count :window (bars 1)))   ; fires in the trailing window
```

- Same previous-tick register rule as every other read: counts fires whose
  timestamps land in `(now - window, now]` as of the end of the previous tick.
- Window is capped by the 256-entry history; reads beyond it return the
  history-bounded count (defaults-inert, never nil).
- This is the only engine delta in this spec outside the overlay itself.

### 4.4 Host-side actions

```lisp
(graph-commit-deltas! name)   ; fold deltas into authored overrides, one undoable edit, then clear
(graph-clear-deltas! name)
(graph-delta-leak! name factor)
```

`graph-commit-deltas!` is the "I like where it drifted" gesture — the
song-mode capture model applied here: ephemeral performance state promoted to
authored state only on request. It writes through the normal override path so
it is a single undo entry and scenes capture the result. Callable from UI
button handlers; **not** callable from process `:run` (a regulator must not
promote its own state — that collapses the layer separation).

## 5. UI

Deliberately minimal; the authored knobs never move.

- **Knobs/matrix unchanged.** They stay bound to authored values via
  `bind-graph`. No ghost markers in v1 — the interesting signal is the
  correction itself, shown separately.
- **Delta matrix viz**: one read-only `matrix` widget next to the existing
  energy/trigger matrices, rendering the edge-weight deltas (diverging scale,
  zero = neutral). Node-field deltas render as a narrow per-node column
  (sum of |delta| per node) — enough to see *where* the regulator is leaning.
  Data rides the existing graph-visualization snapshot path (same as
  `:energy-matrix`).
- **Two buttons** on the graph panel: `commit` (`graph-commit-deltas!`) and
  `clear` (`graph-clear-deltas!`).
- Optional later: per-knob drift tint when `|delta| > epsilon`. Not v1.

## 6. Implementation seams

| Piece | Where |
|---|---|
| Delta store + leak + lifecycle clears | scheduler-owned live graph state (alongside energy accumulators); leak on the same step boundary as `:energy-decay` |
| Resolution | wherever `runtime_config_with_overrides` values feed the live runtime, plus the live-update path in `lisp_host/graph_update.rs` so nudges apply without a rebuild |
| `GraphNudge` command | new variant in the process command enum; applied where the scheduler drains `ProcessRunResult.commands` |
| Natives | `lisp_host/graph_authoring.rs` (writes/reads/actions), read-family extension where `read (track ...)` resolves |
| `:fire-count` | expose the existing 256-entry trigger history with a window count |
| Delta viz | extend the graph visualization snapshot with `:delta-matrix` / `:node-delta-column` |
| Commit | fold via the existing override setters inside one undo group |

Non-goals for v1: delta-able config fields, per-key leak rates, delta
persistence, `def-conductor` determinism tier, autocomplete/hover tooling
(separate spec once this demo surfaces the concrete pain points).

## 7. The demo, sketched

Target script: `crates/sequencer/scripts/processes/graph-homeostat-demo.lisp`
(not yet written — this section is the design sketch). Assumes the
variable-reset graph loaded and routed to track 0, per its own demo file.

The demo is two self-clocked brains (`:every`, `start` — the landed
standalone-brain shape from the Phase 7 demo). Conductor `:observe`
attachment is unnecessary here: bar-granularity regulation doesn't care about
same-tick ordering. System 3 publishes its strain on a channel; System 4
subscribes.

```lisp
;; ── System 3: the homeostat ─────────────────────────────────────────────
;; Once per bar: measure density (fires/bar) and pitch spread on the routed
;; track, compare against the viability envelope, nudge the graph.

(def gh-name "neural-variable-reset-demo")
(def gh-nodes 8)

(defchan gh-strain 0)   ;; how hard System 3 is working, for System 4

(def gh-recent-pitches (track k)
  ;; last k fired transposes, event-locked
  (map (lambda (i) (read (track 0 :transpose :trigs-ago i)))
       (range 1 (+ k 1))))

(def gh-spread (xs)
  (- (apply max xs) (apply min xs)))

(def-process graph-homeostat
  :doc "System 3: hold the graph inside a density + pitch-spread envelope by
        nudging edge weights, node delays, and node transposes."
  :in ((density-lo :int 0 32 :default 3)
       (density-hi :int 0 32 :default 9)
       (spread-min :int 0 48 :default 12)
       (gain :float 0 1 :default 0.25 :lane true))
  :state ((strain 0))
  :every (bars 1)
  :run
  (let ((fires (read (track 0 :fire-count :window (bars 1))))
        (spread (gh-spread (gh-recent-pitches 8))))
    (do
      ;; ── density regulation ──
      ;; Too quiet: feed the loops (weights up), tighten delays.
      ;; Too busy: starve the loops, stretch delays.
      (let ((err (if (< fires (in :density-lo))
                   (- (in :density-lo) fires)
                   (if (> fires (in :density-hi))
                     (- (in :density-hi) fires)   ;; negative
                     0))))
        (if (= err 0)
          (set! strain (* strain 0.5))            ;; envelope held: relax
          (do
            (set! strain (+ strain (abs err)))
            (for-each
              (lambda (n)
                (do
                  ;; ring-neighbor weights carry the pulse; nudge those
                  (graph-nudge-edge! gh-name
                    :from n :to (mod (+ n 1) gh-nodes)
                    :weight (* err 0.02 (in :gain)))
                  ;; delays move opposite to density error
                  (graph-nudge-node! gh-name n
                    :delay (* err -0.15 (in :gain)))))
              (range 0 gh-nodes)))))
      ;; ── pitch-spread regulation ──
      ;; Spread collapsed: fan node transposes outward from the center,
      ;; alternating direction per node so the register opens both ways.
      (if (< spread (in :spread-min))
        (for-each
          (lambda (n)
            (graph-nudge-param! gh-name n :transpose
              (* (if (= (mod n 2) 0) 1 -1)
                 (- (in :spread-min) spread)
                 0.1 (in :gain))))
          (range 0 gh-nodes))
        nil)
      (send :gh-strain strain))))

;; ── System 4: the restructurer ──────────────────────────────────────────
;; Every four bars: is System 3 coping? Sustained strain means the nudge
;; controller has saturated — the envelope is unreachable from this region
;; of the patch. Intervene structurally: clear the accumulated regulation
;; and kick the topology to a new operating point (cross-ring shortcuts +
;; a threshold drop), then let System 3 re-regulate from there.

(def-process graph-restructurer
  :doc "System 4: watch System 3's strain; on saturation, restructure."
  :in ((patience :int 1 8 :default 2)
       (kick :float 0 1 :default 0.6))
  :state ((hot-windows 0)
          (regime 0))
  :every (bars 4)
  :run
  (let ((strain (read (channel :gh-strain))))
    (if (and strain (> strain 6))
      (do
        (set! hot-windows (+ hot-windows 1))
        (if (>= hot-windows (in :patience))
          (do
            ;; regime change: audible, deliberate, still only deltas —
            ;; stop + restart returns to the authored patch.
            (set! regime (+ regime 1))
            (set! hot-windows 0)
            (graph-clear-deltas! gh-name)
            (for-each
              (lambda (n)
                (do
                  ;; open a shortcut across the ring, rotating per regime
                  (graph-nudge-edge! gh-name
                    :from n :to (mod (+ n 3 regime) gh-nodes)
                    :weight (* 0.3 (in :kick)))
                  ;; and lower the firing bar so the new paths can take
                  (graph-nudge-param! gh-name n :threshold
                    (* -0.1 (in :kick)))))
              (range 0 gh-nodes)))
          nil))
      (set! hot-windows (max 0 (- hot-windows 1))))))

;; ── wire and start ──────────────────────────────────────────────────────
(def gh-system3 (graph-homeostat :gain (~slider 0.25)))
(def gh-system4 (graph-restructurer))
(start gh-system3)
(start gh-system4)
```

What the demo demonstrates, in order of appearance to a listener:

1. **Regulation** — mute half the seed rows in the UI; within a bar or two
   the weight-delta matrix visibly warms and density climbs back into the
   envelope. Un-mute; the deltas leak away.
2. **Conflict-free authoring** — drag knobs while it runs; authored values
   and deltas compose, nothing fights.
3. **Escalation** — set the envelope somewhere the patch can't reach
   (density 20–30). Watch strain accumulate, then System 4's regime kick:
   an audible restructure, repeated with a rotating shortcut until the
   envelope is reachable or the listener intervenes.
4. **Safety** — hit stop, restart: the authored patch plays, untouched.
   Like where it wandered? `commit`.

## 8. Open questions

- **Scene change**: clear deltas (spec'd) vs. carry them across so a scene
  switch mid-regulation stays smooth. Clearing is conservative and matches
  "scene = authored state"; revisit if it feels jarring live.
- **`graph-clear-deltas!` from `:run`**: allowed above (System 4 uses it).
  If it proves too sharp a tool, restrict processes to a scoped
  `graph-clear-deltas!` that only clears keys the calling process wrote.
- **Strain metric**: v1 is an ad-hoc accumulator in System 3's state. A
  principled alternative (windup magnitude: sum of |delta| across the
  overlay, read via a `graph-delta-total` native) would decouple System 4
  from System 3's internals — more VSM-honest. Cheap to add later.
- **Leak shape**: multiplicative-only for v1. If slow leaks on int fields
  feel sticky near zero, add a small linear term.
