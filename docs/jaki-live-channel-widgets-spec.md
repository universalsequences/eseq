# Jaki Live Channel Widgets: `:chan`

Status: design, rev 2. Phase 1 (section 7 steps 1-3) is built —
`eseq-jo7.8`, commit `12ebc56e`. Everything else is unbuilt. Extends
`docs/inline-code-widgets-spec.md` (the widget layer) and
`docs/process-channels-spec.md` (the channel layer), and it subsumes the
surface half of bead `eseq-jo7.5`.

**Rev 2 changes (2026-08-21), all from design review before phase 3 started:**

- The `~chan` widget form is **deleted**. Channel-ness is a `:chan` keyword on
  the existing widget forms instead (section 4). `~chan` named the wrong axis:
  it forced a parallel widget registry through `:as`, which would drift from
  `is_inline_widget_constructor_name` and could never express `~lane`.
- Section 7 step 4 is corrected. Binding via
  `attach_inline_widget_runtime_target(callee, inlet, …)` **cannot work** and
  would have blocked `eseq-jo7.10` outright; binding is by source identity
  instead (section 7.4).
- Declaration semantics are specified: `:chan "name"` implicitly and
  idempotently declares, with a seed rule for re-declaration (section 4.4).
  This is not a convenience — without it there is no handle to bind and the
  channel is destroyed on the next `sync_channels` (section 4.4).
- Section 8 gains the scheduler → UI mirror, with **no** drag suppression.

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
  **Rev 2 correction:** it resolves through `recent_runtime_inline_widgets`,
  which `register_inline_widget` populates only when
  `!registering_static_inline_widget`. Statically-drawn widgets are therefore
  unreachable through it — see §7.4, which is why this is not the binding
  mechanism.
- `register_static_inline_widget_expr` (`vm.rs:4959`) walks *source* and
  registers widgets for forms that never execute at authoring time. This is
  why sliders render inside `(jak ...)` at all — the body is quoted and never
  evaluated on the editor VM. It computes a `(callee, inlet)` parent only when
  the widget's immediately preceding sibling is a `Keyword`, so widgets in
  head-argument positions like `(dashdecay …)` have none.
- `inline_widget_source_identity` (`vm.rs:1151`) is
  `(source_revision, start_byte, end_byte)` and *is* recorded for statically
  drawn widgets. It is the viable binding key (§7.4).
- `set_inline_widget_live_value` (`crates/eseqlisp/src/buffer.rs:507`) sets the
  widget's display value and recomputes `__inline-live-diverged` as
  `__inline-text-value != value`. It never fires `on-change`, and returns
  `false` early when nothing changed. This is what makes §8.1's unconditional
  mirror both loop-free and free in the no-conflict case.
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

### 4.1 The surface

```lisp
(jak "hit" :16
  (fig (- . )
    (dashdecay (~slider 0.294 :chan "hit.dashdecay"))
    (dotdecay  (~slider 0.177 :chan true))
    (* (~knob 3 :min 1 :max 4 :step 1 :chan true))))
```

Channel-ness is a **property of an existing widget**, not a new widget form.
`:chan` on any value widget — `~slider`, `~knob`, `~toggle`, and anything added
later — declares a value channel that the pattern reads instead of the literal.
Everything else about the widget is unchanged: same renderer, same inline
width, same min/max/step handling, same literal writeback.

```
(~slider <default> [:chan true | "public-name"] [:min m] [:max x] [:step s])
```

- `<default>` is a bare literal. It is what the widget writes back to on drag,
  what the channel is seeded with, and what the pattern uses if the channel is
  somehow unset. The file remains readable and reloadable as plain data.
- `:chan true` is an anonymous channel, positionally named (§4.3).
- `:chan "name"` opts into a public, global name a process can also read or
  send to (§4.2, §4.4). This is the form that makes a slider and a
  `def-process` two writers on one value.
- Absent `:chan`, the widget behaves exactly as it does today: literal
  writeback only, no live path, an eval to take effect.

**Why a property and not a `~chan` form.** There are two independent choices —
which control to render, and whether the value is live-wired. A `~chan` form
names itself after the second and then has to reintroduce the first as `:as`,
creating a second widget registry running alongside
`is_inline_widget_constructor_name` that will drift from it, and which cannot
express `~lane` at all. As a property, every value-producing widget is
live-capable for free, now and for anything added later, and `~toggle` works on
day one rather than being "reserved for v1".

**Why `:chan` takes a value and is not a bare flag.** The widget argument
convention is strictly pair-wise: `keyword_string_arg` uses `args.windows(2)`
and the property-extraction loop in `register_inline_value_widget_natives`
steps `index += 2`. An unpaired keyword does not get ignored — it
desynchronizes every pair after it, so `(~slider 0.294 :chan :min 1 :max 4)`
would parse as `(:chan, :min)`, `(1, :max)` and produce silent nonsense.
`:chan true` also matches the existing `:lane true` convention on process
inlets, so it does not read as foreign.

**Rejected: implicit promotion inside a `(jak ...)` body.** Every scalar
argument position in a jaki body is channel-capable — that is precisely what
Tier A and Tier B cover between them (§5) — and a value widget cannot legally
sit anywhere else, so a marker carries no information there and could in
principle be dropped. It was rejected for v1 on cost, not on principle:
implicit promotion requires a mechanism that does not exist, namely a way for
`alez.jaki` to declare to the editor VM that its body is a live-widget scope,
plus threading that flag down `register_static_inline_widget_expr`. `:chan`
requires no new VM machinery at all beyond §7.4, which both designs need. The
direction is also one-way — a marker can later become optional inside declared
bodies, on evidence; implicitness cannot be withdrawn once authors rely on it.
Revisit if, in practice, every jaki slider carries `:chan` and no one ever
wants one without.

### 4.2 The primitive

The desugared form is the `eseq-jo7.5` surface, unchanged:

```lisp
(dashdecay (chan "hit.dashdecay" 0.294))
```

`(chan name default)` is authorable directly and is what you write when the
value is driven by a process and there is no slider at all. A `:chan` widget
rewrites to it. Everything in §5 and §6 is specified on `(chan ...)`; the
widget surface inherits it.

### 4.3 Naming

`jak` walks its quoted body at macro-expansion time (editor VM) and rewrites
each `:chan`-carrying widget form into `(chan <name> <default>)`, assigning
`"<seq-name>#<n>"` for anonymous (`:chan true`) occurrences, where `n` is a
pre-order index over the body. The same walk performs the widget→channel
binding (§7.4).

The rewrite is load-bearing, not cosmetic. The scheduler VM has no widget
layer: `register_inline_value_widget_natives` returns the first argument
unchanged when `inline_widget_registration_enabled()` is false, so an
unrewritten widget form would silently evaluate to its bare literal and no
channel would exist.

**Two namespaces with different sharing semantics, deliberately.** Anonymous
`"<seq-name>#<n>"` names are scoped per sequencer and are an implementation
detail. Explicit `:chan "name"` names are **global and flat** — that is exactly
what makes a slider and a `def-process` able to meet on one value, and it also
means the same `:chan "decay"` in two different bodies is silently one control.
That is the intended trade, but it must be stated: an explicit name is a public
declaration, not a local label.

Positional naming is stable between evals and re-derived on every eval, so it
is always internally consistent. Inserting an anonymous `:chan` above another
one renumbers the ones below it, which resets those channels to their literal
defaults on the next eval. That is acceptable — the literal is correct by
construction — and `:chan "name"` is the escape hatch for anything you want to
keep across restructuring or share with a process.

Rejected alternative: deriving names from editor anchor ids. Anchors are stable
across edits, but they do not exist on the scheduler VM, which sees only the
quoted body. Any anchor-derived scheme requires smuggling editor state into the
pattern source, which breaks the "the file is the truth" property.

### 4.4 Declaration is implicit, idempotent, and required

The `jak` walk **declares** each `:chan` widget's channel: it pushes an
`AuthoredChannel { handle_id, name, initial: <literal>, message_only: false }`
onto the authoring registry, exactly as `defchan` does. If a channel with that
name already exists, the walk **reuses it and its `handle_id`** rather than
creating a second one.

This is not a convenience. Two things break without it:

1. **There is no handle to bind.** The widget writes through a channel handle,
   and `process_channel_handle` resolves its name via `channel_name_for_handle`
   (`process_dsl_parse.rs`), which reads `registry.channels` and
   `registry.channel_handles` — both populated only by declaration. `defchan`
   is currently the sole producer of channel handles in the system. Undeclared,
   §7.4 has nothing to attach and phase 3 cannot be built at all.
2. **The channel is destroyed on the next republish.** `sync_channels`
   (`runtime/process.rs`) builds a fresh map from the authored list and ends
   with `self.channels = next`. It preserves the runtime *value* of a channel
   that is in that list — `value.or(channel.initial)`, which is the rule that
   keeps a dragged value alive across an eval — but silently drops anything
   absent from it. A channel conjured on demand by `send_channel_at`'s
   `.entry(name).or_insert(…)` is exactly that. The first `sync_authoring`
   after any eval would evaporate the dragged value.

Reuse-by-name also makes declaration idempotent, which matters because
`defchan` today does `registry.channels.push(…)` rather than an upsert, so
re-evaluating a buffer appends a duplicate entry every time. Harmless —
`sync_channels` collapses by name and both handles resolve to the same name —
but it grows. Named channels declared through `:chan` must not inherit that,
which removes them from §10's leak question entirely and leaves only the
anonymous positional ones, bounded by body size and scoped per sequencer.

**Seed rule on re-declaration.** Reuse leaves open what happens to `initial`
when the literal in the buffer has changed. Compare the new literal against the
**stored** `AuthoredChannel.initial`, never against the runtime value:

- **Differs** — the author edited the text by hand. Update `initial` and push
  the new value through as a normal channel write on the §7 pending-write
  queue. No `sync_channels` change is needed and the write lands on a chunk
  boundary with a defined beat like every other one.
- **Matches** — an unchanged re-eval. Leave the runtime value alone.

Checked against every case:

| situation | stored `initial` | literal | action | correct because |
|---|---|---|---|---|
| after a drag (no eval yet) | 0.294 | 0.9 | force 0.9 | runtime is already 0.9; a no-op |
| hand-edited literal | 0.294 | 0.5 | force 0.5 | the text is the author's intent |
| process drove it, text unchanged | 0.9 | 0.9 | preserve | an eval must not stomp a live process value |
| plain re-eval after a drag | 0.294 | 0.9 | force 0.9 | runtime is already 0.9; a no-op |

Comparing against the runtime value instead would fail row 2 — the runtime
value equals the old literal, so a hand edit would appear to change nothing
until the app restarted, which is precisely the "what you hear and what you
read disagree" failure §8 exists to prevent.

**Message-only collision is an error.** `defchan` with a single argument
creates a message-only channel, which holds no value at all —
`send_channel_at` explicitly skips storing into it. A widget always supplies a
literal and therefore always wants a value channel. Binding `:chan "ping"` to
an existing `(defchan ping)` must be a hard error at expansion time, not a
silent reuse: the symptom would otherwise be a slider that moves smoothly and
does nothing.

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
4. **Widget target binding.** See §7.4 — the mechanism named in rev 1 does not
   work.

The whole live path is thus: drag frame → existing `runtime.invoke` →
channel handle → queue → chunk drain → `send_channel_at` → snapshot →
`chan-get` at tick. No evaluation at any point.

### 7.4 Target binding must be keyed by source identity

Rev 1 specified that the `jak` walk call
`attach_inline_widget_runtime_target(callee, inlet, target)` with the channel
handle and `"set"`. **That cannot work,** for two independent reasons. This was
found before phase 3 started; as written it would have blocked `eseq-jo7.10`
outright.

1. **Static widgets never populate the lookup table.** That function resolves
   through `recent_runtime_inline_widgets`, keyed by `(callee, inlet)`. The map
   is written in `register_inline_widget` only under
   `(!self.registering_static_inline_widget)`. The documented intent is that
   the static pass draws the widget and a *later execution* of the same form
   re-registers under the same source identity carrying the runtime parent —
   which is how process call sites attach targets. A jaki body is quoted and
   never executes on the editor VM, so that second registration never happens
   and the lookup always misses.
2. **There is no `(callee, inlet)` pair to key on.**
   `register_static_inline_widget_expr` computes a parent only when the
   widget's immediately preceding sibling is a `Keyword`. In
   `(dashdecay (~slider 0.294 :chan true))`, `dashdecay` is a head symbol and
   the widget sits at index 1, so the parent is `None`. Jaki flags are head
   positions, not keyword arguments — a different shape from the process-inlet
   convention the whole mechanism was built around.

**The fix.** Key on the widget's source identity, which already exists and is
already recorded for every statically-drawn widget:
`inline_widget_source_identity` is `(source_revision, start_byte, end_byte)`.
Either add a span-keyed variant of `attach_inline_widget_runtime_target`, or —
preferably — let the widget native attach its own target at static-registration
time, since with `:chan` the form itself is the thing that knows it is a
channel. The latter dissolves the parent-pair dependency entirely.

**Consequence for `apply_widget_output`: none.** The claim that no new code is
needed in the widget interaction path still holds. Only the binding changes,
not the write path.

**Corollary — min/max inference does not reach jaki bodies.** §4.1's inherited
"same min/max/step inference" is vacuous here for the same reason as (2):
inference runs through `resolve_inline_widget_metadata(callee, inlet)`, which
needs the same absent pair. Widgets in a jaki body fall back to the default
`0.0..1.0` range unless `:min`/`:max` are given explicitly. Fine for
`dashdecay`; wrong for anything else, which is why the §4.1 example writes
`(~knob 3 :min 1 :max 4 :step 1 :chan true)` out in full.

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
tracks `__inline-live-diverged` (`buffer.rs:535-553`). `:chan` reuses it
unchanged.

**Precedence when a process also writes.** Last writer wins, with no special
casing for the UI. `send_channel_at` is one queue; the slider is one more
writer on it. An author who wants a slider to win uninterrupted should not
point a process at the same name.

### 8.1 The scheduler → UI mirror

A named channel is bidirectional by design: a `def-process` can read or write
the same value a slider drives. The slider→process direction works as of phase
1 — `send_channel_at` calls `propagate_source_at`, which writes patched
`ProcessTargetRef::Inlet` targets directly and then walks every running
instance's `listens` via `listener_invocations_for_source`, and the §7 drain
feeds the resulting invocations into the lookahead cascade.

The reverse direction does not move the slider today. Phase 1's
`__inline-read` returns an echo of the author's last write, falling back to the
declared initial, and deliberately does not read the scheduler-side value. To
make a process visibly drive a slider, add the mirror of §7: a published
channel-value snapshot on `SequencerState` going scheduler → UI, and have
`__inline-read` prefer it over the echo. The polling half already exists —
`refresh_inline_widget_runtime_values` invokes `__inline-read` per binding and
pushes results through `set_inline_widget_live_value`.

**There is no drag suppression, and there must not be.** The obvious guard —
suppress the mirror while a widget is actively being dragged, so a process
cannot fight the author's hand — is wrong on four counts.

- It is the UI special-casing this section just ruled out, relocated into the
  display layer.
- It defers the conflict rather than resolving it. Release the drag and the
  process's value snaps the slider away from where you put it, which is worse
  than seeing the fight while it happens.
- "Actively dragging" is mode state that gets stuck. Lost pointer capture, a
  focus change, a modal opening mid-drag — each leaves a slider permanently
  deaf to the mirror.
- It breaks the divergence indicator. `set_inline_widget_live_value` recomputes
  `diverged` on each update; a suppressed widget does not update, so the flag
  goes stale exactly when it matters.

Always mirroring is safe and, in the common case, invisible.
`set_inline_widget_live_value` mutates only the widget map's `value` and the
divergence flag and never fires `on-change` — the write path is
`apply_widget_output`, which runs only from real interaction — so there is no
feedback loop. It is idempotent, returning `false` early when both value and
flag are unchanged, so mirroring your own value back costs no revision bump and
no re-render. And it computes `diverged` as `__inline-text-value != value`,
comparing the live value against the *literal in the source* rather than
against the last write, which is the correct semantic here and self-clears when
they reconverge.

So the slider moves under your hand only when a process genuinely is contending
for the value — which is a thing to surface immediately, and which the diverged
indicator names.

**Transport-stopped.** Writes apply while stopped. The next tick after
transport start reads the current value; there is no queued backlog.

## 9. Phasing

Each phase is independently useful and independently shippable.

1. **UI → channel write path** (§7 steps 1-3). ✅ **Built** — `eseq-jo7.8`,
   commit `12ebc56e`. Testable on its own with an existing `defchan` and a
   `chan-get` in any generator tick. No jaki changes, no widget changes.
2. **Tier A late resolution** (§5 Tier A, §6 payload-epoch). Authorable via the
   bare `(chan name default)` primitive. At the end of this phase a process can
   already drive `dashdecay` live.
3. **`:chan` surface** (§4, §7.4). Splits cleanly along the crate boundary into
   two independently reviewable pieces:
   - 3a, jaki side: the `jak` expansion walk — recognizing `:chan`, positional
     naming, declaration and reuse with the §4.4 seed rule, and the rewrite to
     `(chan name default)`.
   - 3b, eseqlisp side: `:chan` on the value widget natives and source-identity
     target binding (§7.4).
   This is the phase the author feels.
4. **Tier B early resolution** — `eseq-jo7.5` as already specified, extended
   with the static tier assignment in §5.
5. **Memo cap tuning** against the scheduler-tick probe.
6. **Scheduler → UI mirror** (§8.1). Optional and separable: phases 1-4 give a
   slider that drives a process; this is what lets a process drive the slider.

Phases 1-3 deliver the whole experience for payload arguments. Phase 4 is what
makes the structural sliders in the motivating screenshot correct rather than
merely responsive.

## 10. Open questions

1. ~~**Does `~chan` deprecate `~slider` inside `(jak ...)`?**~~ **Resolved in
   rev 2** — the question dissolved with `~chan`. A plain `~slider` is now the
   same form without `:chan`, so both coexist with no ambiguity and the
   "silent promotion" option is recorded as rejected-on-cost in §4.1.
2. **Channel lifetime.** Anonymous `"<seq-name>#<n>"` channels accumulate in
   the authoring registry across evals as bodies are edited and renumbered.
   Needs a sweep keyed on the owning sequencer name, or they leak. Reduced in
   scope by rev 2: §4.4 reuse-by-name makes *named* channels idempotent, so
   only the anonymous ones churn, and only for `:chan`-marked widgets. Still
   required before phase 3 ships, not after.
3. **`(cyc ...)` composition.** Is `(chan ...)` legal inside a `(cyc ...)` arm,
   and is `(cyc ...)` legal as a `:chan` widget's default? Both are
   representable; v1
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
- **Explicit names collide silently.** `:chan "name"` is global and flat (§4.3)
  — that is what makes slider/process sharing work, and it means the same name
  in two bodies is one control with no warning. Mitigation is naming
  discipline, not a mechanism; a lint could list duplicate explicit names if it
  proves to be a problem in practice.
- **Declaration is now a side effect of editing pattern text.** Channels are
  created and reused as `:chan` widgets appear and are renamed (§4.4), so the
  authoring registry churns with editing in a way it did not when `defchan` was
  the only producer. Reuse-by-name bounds the named case; §10.2's sweep is
  required for the anonymous one before phase 3 ships.
