# Jaki Live Channel Widgets: `~chan`

Status: design. Nothing below is built. Supersedes nothing; it extends
`docs/inline-code-widgets-spec.md` (the widget layer) and
`docs/process-channels-spec.md` (the channel layer), and it subsumes the
surface half of bead `eseq-jo7.5`.

## 1. The problem

Inline widgets currently render inside a `(jak ...)` body and are draggable,
but the value only reaches the running sequencer when the author manually
re-evaluates the form. That is a full buffer eval: macro expansion, a new
`def-sequencer` publish, a fresh quoted body, and all three jaki memos thrown
away. It also lands on `eseq-wlc` (buffer re-eval causes momentary timing
wobble). The result is that a slider — the most direct control surface in the
editor — is the least direct thing in the system: grab, drag, release, switch
hands to the keyboard, eval, hear it.

The nearest existing mechanism is the process inlet write-through: a widget
that resolves a runtime target writes live, every drag frame, with no eval at
all. Jaki widgets resolve no target, so they have no live path.

This spec gives jaki a live path, and gives it a surface form that is shorter
than the one it replaces.

## 2. Goals

- Dragging a slider inside a `(jak ...)` body changes what you hear, now,
  without evaluation.
- The literal in the buffer stays the source of truth for persistence — the
  file still reads as plain data and still reloads to the same sound.
- Structural arguments (the ones that change cycle length or phase) remain
  memo-correct. No silent staleness, ever, and no drift.
- The same slot a slider drives can also be driven by a process, without
  changing the pattern text.

### Non-goals

- Removing manual eval. Editing pattern *shape* (`(- . . )`, route words, new
  figures) is still an eval. This spec is about scalar arguments only.
- Automation recording, MIDI learn, or channel curves over time. Later.
- Changing `def-sequencer` publishing or the quoted-body model.

## 3. Verified repo facts

These ground the design; each was read on 2026-08-21.

**Widget layer**

- `apply_widget_output` (`crates/eseqlisp/src/editor/mod.rs:8885-8925`) does
  two independent things per drag frame: if the widget carries
  `__inline-runtime-target` plus `__inline-parent-inlet` it calls
  `runtime.invoke(target, [:inlet, value])` live; separately it rewrites the
  literal via `Buffer::write_inline_widget_value`
  (`crates/eseqlisp/src/buffer.rs:599`).
- The text writeback does **not** trigger evaluation.
  `pending_inline_writeback` (`editor/mod.rs:697, 8916`) is undo coalescing
  only. So today's "recompile on release" is the author's own eval keystroke,
  not a side effect of the drag.
- `Runtime::invoke` (`crates/eseqlisp/src/runtime.rs:2726`) forwards to
  `vm.invoke(callable, args)` and accepts any callable, including a native
  closure — the target does not have to be a process instance.
- `attach_inline_widget_runtime_target` (`crates/eseqlisp/src/lang/vm.rs:5013`)
  is how a target gets bound, keyed by `(callee, inlet)`; the only production
  caller is the process constructor
  (`crates/sequencer/src/lisp_host/eseq/process_natives.rs:2698`).
- `register_static_inline_widget_expr` (`vm.rs:4959`) walks *source* and
  registers widgets for forms that never execute at authoring time. This is
  why sliders render inside `(jak ...)` at all — the body is quoted and never
  evaluated on the editor VM.
- `~slider`/`~knob`/`~toggle` are registered by
  `register_inline_value_widget_natives` (`crates/eseqlisp/src/widgets.rs:103`).
  Each returns its first argument unchanged when inline registration is
  disabled — which is exactly what happens on the scheduler VM.

**Channel layer**

- `chan-get` (`crates/sequencer/src/lisp_host/eseq/sequencer_natives.rs:759`)
  reads a per-chunk snapshot, is generator-tick scoped, and is read-only.
- The snapshot is published once per chunk at
  `crates/sequencer/src/scheduler/lookahead.rs:1458-1461` from
  `ProcessRuntime::channel_values` (`crates/sequencer/src/runtime/process.rs:1817`).
- `ProcessRuntime::send_channel_at` (`runtime/process.rs:2241`) is the correct
  write entry point and already does propagation. **Every caller is internal to
  the process runtime.** No UI path exists.
- `sync_channels` (`runtime/process.rs:1790`) preserves an existing runtime
  value across authoring re-publish and falls back to `initial` only when
  unset. A UI write routed through the authoring snapshot would therefore be
  swallowed — this is why §7 uses a queue, not the authoring path.
- `defchan` (`process_natives.rs:225`) creates an `AuthoredChannel` and returns
  a handle from `process_channel_handle`
  (`process_dsl_parse.rs:1418`), whose callable is **currently a no-op stub
  returning `true`**. It is a ready-made seam.
- `defchan` with one argument is message-only (`process_natives.rs:243`); a
  value channel requires an initial.

**Jaki layer**

- `jak` (`content/packages/alez.jaki/src/surface.lisp:34`) expands to
  `def-sequencer` with the body shipped as quoted source, run on the scheduler
  VM.
- `resolve-arg` (`content/packages/alez.jaki/src/core.lisp:208`) resolves
  `(cyc ...)` and falls back to `(eval (source raw))` for everything else. This
  is the hook point for late resolution.
- Payload flags (`basevel`, `dotdecay`, `dashdecay`, `minvel`, `maxvel`) are
  resolved in `vel-overrides` (`core.lisp:342-355`), which runs **inside**
  `eval-at` and therefore inside the `eval-cycle` memo.
- Three memos: `memo-store` keyed by `(:id cycle hand st...)` capped at 15
  (`core.lisp:653`); `len-memo` keyed by `(:id k)` capped at 31
  (`core.lisp:664`); `lens-memo` keyed by `:id` capped at 23 (`core.lisp:712`).
- `:id` is `(source body)` plus transform tags (`core.lisp:156, 172, 179, 193`).
- `arg-period` (`core.lisp:673`) treats every non-`cyc` form as period 1. A
  value that changes without changing `:id` therefore produces a **wrong
  super-cycle table**, silently.

## 4. Surface

### 4.1 The sugar

```lisp
(jak "hit" :16
  (fig (- . )
    (dashdecay (~chan 0.294))
    (dotdecay  (~chan 0.177))
    (* (~chan 3 :min 1 :max 4 :step 1))))
```

`~chan` is a slider that is also a channel. It renders exactly as `~slider`
renders today (same widget, same inline width, same min/max/step inference,
same literal writeback), and it additionally declares a value channel that the
pattern reads instead of the literal.

Signature:

```
(~chan <default> [:name "public-name"] [:min m] [:max x] [:step s] [:as :knob])
```

- `<default>` is a bare literal. It is what the widget writes back to on drag,
  what the channel is seeded with, and what the pattern uses if the channel is
  somehow unset. The file remains readable and reloadable as plain data.
- `:name` opts into a public, stable channel name that a process can also
  `send` to. Omitted, the channel is anonymous (§4.3).
- `:as :knob` selects the knob renderer; default is the horizontal slider.
  `:as :toggle` is reserved and not in v1.

`~chan` is deliberately not a new widget. It is `~slider` plus an identity.

### 4.2 The primitive

The desugared form is the `eseq-jo7.5` surface, unchanged:

```lisp
(dashdecay (chan "hit.dashdecay" 0.294))
```

`(chan name default)` is authorable directly and is what you write when the
value is driven by a process and there is no slider at all. `~chan` expands to
it. Everything in §5 and §6 is specified on `(chan ...)`; `~chan` inherits it.

### 4.3 Naming

`jak` walks its quoted body at macro-expansion time (editor VM) and rewrites
each `(~chan ...)` into `(chan <name> <default>)`, assigning
`"<seq-name>#<n>"` for unnamed occurrences, where `n` is a pre-order index over
the body. The same walk hands the editor the widget→channel binding.

Positional naming is stable between evals and re-derived on every eval, so it
is always internally consistent. Inserting a `~chan` above another one
renumbers the ones below it, which resets those channels to their literal
defaults on the next eval. That is acceptable — the literal is correct by
construction — and `:name` is the escape hatch for anything you want to keep
across restructuring or share with a process.

Rejected alternative: deriving names from editor anchor ids. Anchors are stable
across edits, but they do not exist on the scheduler VM, which sees only the
quoted body. Any anchor-derived scheme requires smuggling editor state into the
pattern source, which breaks the "the file is the truth" property.

## 5. Two-tier resolution

Jaki arguments are not one class, and the distinction is load-bearing.

### Tier A — payload arguments (late resolution)

`basevel`, `dotdecay`, `dashdecay`, `minvel`, `maxvel`, and the route-word
`vel`/`note` arguments. These affect emitted event payload only. They do not
change the symbolic event grid, the cycle length, or the phase.

Resolution: `resolve-arg` gains a `chan` head alongside `cyc`, returning
`(chan-get name default)`. The value is read at tick time. `:id` is unchanged,
`len-memo` and `lens-memo` stay valid, no pattern is rebuilt, and no evaluation
happens anywhere. This is the case where "the slider is a wire" is literally
true.

This is the majority of what an author reaches for a slider to grab, and it is
the tier that makes the feature feel instant.

### Tier B — structural arguments (early resolution)

`(* n)` / `(/ n)` / `(% n)` in a figure, `fast`/`slow`, `shift`, `rot`,
`trunc`, `every`, and `align`. These change length and phase.

Resolution: a `resolve-chans` tree-walk at the top of `alez.jaki.core/run`
substitutes each `(chan name default)` in the quoted body with the channel's
current value **before** pattern construction, so the literal lands in
`(source body)` and therefore in `:id`. Every distinct value is a distinct
pattern; all three memos are automatically correct because they are keyed on
`:id`. This is `eseq-jo7.5` as already specified in the bead.

Cost: a pattern rebuild per distinct value. Bounded and cheap relative to a
buffer eval, but a drag generates many distinct values, so §6 raises the memo
caps.

### Tier assignment is static and mechanical

The tier of a `(chan ...)` is determined by its syntactic position, not by a
user annotation. `resolve-chans` knows which positions are structural because
those are exactly the positions `fig-period` / `xf-period` / `arg-period`
consult. A `(chan ...)` in a structural position is substituted early; one
anywhere else is left for late resolution. There is no way for an author to get
this wrong, and no way for a new structural position to be added without
`arg-period` learning about it in the same commit.

## 6. Memo invalidation

Tier A is not free, because `vel-overrides` runs inside `eval-at` and therefore
inside the `eval-cycle` memo (`core.lisp:653`). A late channel read behind an
unchanged key would serve a stale velocity forever. Two epochs fix it, with a
deliberate asymmetry:

- **`payload-epoch`** — a global counter bumped whenever any channel value
  changes. It joins the `eval-cycle` memo key. A drag therefore invalidates the
  15-entry per-cycle memo and nothing else.
- **`len-memo` / `lens-memo` are untouched by Tier A.** Payload channels
  provably cannot change length, so the expensive tables — the ones whose loss
  caused the scheduler pinning that `lens-memo` was introduced to fix
  (`core.lisp:706-709`) — survive every drag frame.

Tier B needs no epoch at all: early substitution changes `:id`, which rekeys
all three memos correctly by construction.

The per-drag cost is therefore one cycle re-evaluation per active route, versus
a whole buffer eval today. That is the entire performance argument for the
feature.

Memo caps: raise `memo-store` from 15 and `lens-memo` from 23 to absorb a drag
without thrash. Exact numbers are a measurement, not a design decision; the
scheduler-tick timing probe from `b4411cdf` is the instrument.

A single global `payload-epoch` is intentionally coarse — moving any slider
invalidates the per-cycle memo for every pattern. Per-pattern read sets would
be finer, and are a later optimization if a measurement asks for one.

## 7. Plumbing: UI → channel

The one genuinely missing piece.

1. **Channel handle becomes callable.** `process_channel_handle`
   (`process_dsl_parse.rs:1418`) currently returns `EValue::Bool(true)` for any
   call. Give it a real callable: `(handle :set value)` enqueues a pending
   channel write on the shared authoring registry.
2. **Pending writes queue.** A `pending_channel_writes: Vec<(String, Value)>`
   on the authoring registry, mirroring `pending_step_inlet_writes`
   (`runtime/process.rs:1747`). This must **not** ride the authoring snapshot:
   `sync_channels` (`runtime/process.rs:1790`) prefers the existing runtime
   value over `initial`, so a value smuggled in as `initial` would be dropped.
3. **Scheduler drain.** The lookahead worker drains the queue at the top of a
   chunk and calls `send_channel_at(name, value, beat, sample_time)` before the
   snapshot is published at `lookahead.rs:1461`. Writes therefore land on a
   chunk boundary, in order, with a defined beat — the same discipline every
   other control-thread → scheduler command follows.
4. **Widget target binding.** The `jak` expansion walk (§4.3) calls
   `attach_inline_widget_runtime_target` with the channel handle as the target
   and `"set"` as the inlet, so `apply_widget_output`'s existing live-write
   branch (`editor/mod.rs:8905`) fires unchanged. No new code in the widget
   interaction path.

The whole live path is thus: drag frame → existing `runtime.invoke` →
channel handle → queue → chunk drain → `send_channel_at` → snapshot →
`chan-get` at tick. No evaluation at any point.

## 8. Semantics

**Latch.** Tier A values are read per tick. Tier B values are latched at a
cycle boundary; phase stays transport-locked across a length change, matching
the per-route fast/slow behavior from `ca3342f5`. A structural slider therefore
never flips a length mid-cycle.

**Chunk granularity.** A write is visible to ticks in the *next* chunk, per
`chan-get`'s documented contract. At normal lookahead this is imperceptible for
payload; it is exactly right for structural, which latches at a cycle boundary
anyway.

**Persistence.** The drag writes the channel live *and* rewrites the literal,
independently, as today. Reload replays the literal as the channel's seed. A
file is never in a state where what you hear and what you read disagree after a
reload.

**Divergence.** Between a live write and a text writeback the widget already
tracks `__inline-live-diverged` (`buffer.rs:537-553`). `~chan` reuses it
unchanged: a channel driven by a *process* while a slider also points at it
shows as diverged rather than fighting the author's hand.

**Precedence when a process also writes.** Last writer wins, with no special
casing for the UI. `send_channel_at` is one queue; the slider is one more
writer on it. An author who wants a slider to win uninterrupted should not
point a process at the same name.

**Transport-stopped.** Writes apply while stopped. The next tick after
transport start reads the current value; there is no queued backlog.

## 9. Phasing

Each phase is independently useful and independently shippable.

1. **UI → channel write path** (§7 steps 1-3). Testable on its own with an
   existing `defchan` and a `chan-get` in any generator tick. No jaki changes,
   no widget changes.
2. **Tier A late resolution** (§5 Tier A, §6 payload-epoch). Authorable via the
   bare `(chan name default)` primitive. At the end of this phase a process can
   already drive `dashdecay` live.
3. **`~chan` sugar** (§4, §7 step 4). The `jak` expansion walk, naming, and
   widget target binding. This is the phase the author feels.
4. **Tier B early resolution** — `eseq-jo7.5` as already specified, extended
   with the static tier assignment in §5.
5. **Memo cap tuning** against the scheduler-tick probe.

Phases 1-3 deliver the whole experience for payload arguments. Phase 4 is what
makes the structural sliders in the motivating screenshot correct rather than
merely responsive.

## 10. Open questions

1. **Does `~chan` deprecate `~slider` inside `(jak ...)`?** A plain `~slider`
   there is now strictly worse: same pixels, needs an eval. Options are to
   leave both (confusing), warn on `~slider` inside a jaki body, or silently
   treat it as `~chan`. Silent promotion is tempting and probably wrong —
   `~slider`'s literal-editing semantics are legitimate for a value you want
   frozen into the source.
2. **Channel lifetime.** Anonymous channels accumulate in the authoring
   registry across evals as bodies are edited. Needs a sweep keyed on the
   owning sequencer name, or they leak.
3. **`(cyc ...)` composition.** Is `(chan ...)` legal inside a `(cyc ...)` arm,
   and is `(cyc ...)` legal as a `~chan` default? Both are representable; v1
   should probably forbid the second and allow the first.
4. **Scope of `payload-epoch`.** Global in v1 (§6). Whether per-pattern read
   sets are worth it is a measurement.
5. **Undo.** A live channel write is not in the undo stack; only the literal
   writeback is. Undoing a drag restores the text but not the channel until the
   next eval. Probably fine, possibly surprising.

## 11. Risks

- **Tier misassignment is silent and musical.** If a structural position is
  added to jaki without `arg-period` learning about it, a `(chan ...)` there
  resolves late and the super-cycle table goes quietly wrong — drift, not a
  crash. Mitigation: the tier list and `arg-period` are the same list, and a
  test asserts they agree.
- **Memo thrash under drag.** Tier B rebuilds a pattern per value. If caps are
  too low, a fast drag on a `(* n)` slider could pin the scheduler the way rich
  patterns did before `lens-memo`. Mitigation: phase 5, measured.
- **Two sources of truth during a drag.** Live channel value and buffer literal
  diverge by design between frames. The existing `__inline-live-diverged`
  machinery is the mitigation, but it was built for process inlets and has not
  been exercised on a scheduler-VM-resolved value.
