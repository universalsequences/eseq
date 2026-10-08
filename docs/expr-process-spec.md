# Expr processes

> Names below predate kind bindings (eseq-0l17); see docs/kind-bindings-spec.md.

Status: spec rev 1, 2026-09-27. eseq-waa9.10 (headless core, nodes) and eseq-waa9.11 (card, edit buffer, overflow; nodes) and eseq-waa9.12 (context
variables, direct writes, inlet shadowing) and eseq-waa9.13 (state, stateful helpers) and eseq-waa9.15 (presets, shaping helpers, `->`) and eseq-waa9.16 (`*processes*` dock) and eseq-waa9.17 (promote + edit as expr) BUILT uncommitted 2026-09-27; see the "As shipped" notes. Epic: `eseq-waa9` (graph node
processes); slices are children `eseq-waa9.10`–`eseq-waa9.17` (§11).

## 1. Problem

The node process bay (`docs/graph-node-processes-spec.md`) made chains like
`acc -> delay` playable: an accumulator feeding a node's propagation delay
gives the 90s "bouncing balls" rhythm that speeds up, slows down, resets.
What it cannot do is shape the signal between two cards. `acc -> * k -> sin ->
delay` needs a `*` process, a `sin` process, and so on — a dozen tiny classes
that each cost a `def-process`, a doc string, and a dropdown row, and still
never cover the next idea.

Every process body is already Lisp that ships to the scheduler VM
(`content/processes/builtin.lisp`, `:run`). The missing piece is letting the
player write that body on the spot, for one card, without writing a
`def-process`.

## 2. The expr card

`expr` is one process class in the add-process dropdown. Its card is the same
uniform size as every other card in the bay grid — no oversized text box.

The card shows:

- its title (`expr`) and, as a clipped one-line label, a preview of the body;
- one port per inlet (§3), the same ports every other card has;
- the connectable `wire` output, like `acc` and `rand`;
- an **edit** button.

Pressing **edit** opens an ordinary text-mode buffer named for the slot
(`*expr node 3 · slot 2*` on a node, `*expr track 5 · slot 1*` on a track).
Completion, paren matching and eldoc come from the code editor for free; no
text widget is embedded in the card. Before the sidebar dock (§7) exists the
buffer opens like any other buffer; once it exists, the dock is its default
home.

Committing the buffer (save, or `C-c C-c`):

1. parses the body and derives the inlet set (§3);
2. compiles it into a hashed class (§2.1);
3. reconciles the slot's inlets: names that survive keep their values and
   cables, new names start at 0, removed names drop their cables and a toast
   names them;
4. rebinds the slot to the new class in place (same slot id, same position in
   the chain, same fan-out).

If parsing or compiling fails, the previous class keeps running, the error is
reported in the buffer (span-anchored where the parser gives a span), and the
card shows an error dot. A runtime error inside the body on a fire (§8)
bypasses the card for that fire — the payload passes through untouched — and
sets the same error dot. A typo never silences the groove.

`expr` works on track lane patchbays as well as node bays: the bays are the
same widget (spec eseq-waa9.6) and the class model is shared.

As shipped (eseq-waa9.10, headless): the commit is the native
`(graph-node-process-expr-set seq node slot-id source)`, which returns
`{:ok :class :inlets :removed :error :span}` (`:span` is `(start end)` byte
offsets into the body, or nil; on `:ok false` the slot is untouched and
`:inlets` lists the old class's inlets). `(graph-node-process-expr-source seq
node slot-id)` reads the stored body. The node chain read
(`graph-node-process-chain`) gains `:expr` (bool), `:expr-source` (string or
nil) and `:error` (string or nil: the scheduler's last run error for the slot,
cleared by its next clean run, or "not compiled" when the body's class is
missing); `:label` is `expr`. The shared lane-patch shape
(`graph-node-lane-patch` / `SEQ.track-lane-patch`) is unchanged. (Both are
gone since: the track one in eseq-0l17.66, the node one in eseq-0l17.67; the
patchbays read the kinds' `process` fields.) The removed
inlets also go to the status line (the toast hook for .11). The body's value
is sent by the internal native `__expr-send!`: a number goes out as is, a bool
as 1/0, nil sends nothing, anything else is a run error (bypass). Nodes only:
track lane slots keep their class on the scene-independent roster
(`TrackLaneRosterSlot`) and edit through the recorded history commands, so
the track twin is a follow-up bead; `TrackProcessSlot.expr_source` already
carries the body for both.

As shipped (eseq-waa9.11, node bays): the card keeps the uniform box. Its
title row holds `expr`, a red error dot (theme `:toast-error`, distinct from
the orange enable dot and the port rings) and a small **edit** button; the
body preview rides the out-port row beside the `wire` port, whitespace
collapsed by the host (`:expr-line` on the lane-patch entry) and clipped to
13 chars with `…`. The lane-patch entry (node and track builders alike) gains
`:expr`, `:expr-line` and `:compile-error` (a stored body with no compiled
class). The dot is lit by, in order, `:compile-error`, the slot's last
failed commit (kept per slot by `eseq.expr-buffer`, cleared by a good one)
and the scheduler's last run error, which reaches the UI as the process
kind's `p.error` (pushed by the kinds' tick when
`process_run_errors_version` moves and some process observes it; read
inside the dot's own subtree). Before eseq-0l17.66 it was the
`SEQ.process-run-errors` list. The inspector shows the same message above
the inlets.

The edit buffer lives in `content/ui/expr-buffer.lisp`
(`eseq.expr-buffer/open-node-slot graph node slot-id slot-index`). It is a
scratch buffer in mode `eseq.expr-buffer/expr-mode`: ordinary eseqlisp
text, no live keys. The name is `*expr node N · slot M*` with N as the
node editor labels it and M the 1-based position when opened (a clash
across instances gets ` <2>`); the buffer is tied to the slot instance id,
so reordering does not retarget it and reopening focuses it. Commit keys:
`C-c C-c` and `C-x C-s` (mode chords, which now outrank global chords), the
`save-buffer` command/native, and File > Save (Cmd+S), which commits instead
of saving the project while the active buffer's mode saves itself. This
rests on two general editor features: `define-mode … :on-save "fn"` (save
calls the handler instead of writing a file; a truthy result marks the
buffer saved, false leaves it modified) with the predicate
`(current-buffer-saves-itself?)`, and mode keymaps taking part in chord
dispatch. A failed commit keeps the buffer modified, sets the status line
and an error toast, highlights the reported span (`:where`, below) with a
buffer text style and moves the cursor to its line; a good one toasts
removed inlets. A commit after the slot, node or instance is gone is
refused with a message (`graph-node-process-slot?`, which never raises).
`graph-node-process-expr-set` also returns `:where (line column end-line
end-column)`, 0-based with char columns. `eseq.expr-buffer/commit-source`
commits a body with the same bookkeeping without a buffer (scripts,
fixtures). Track lane cards show the preview and dot but no edit button
until eseq-waa9.18.

### 2.1 Hashed classes

Each distinct body compiles to a hidden process class named
`expr#<hash>`, where the hash covers the normalized source. The class is an
ordinary `def-process` built by the host from the body — the same authoring
registry path (`lisp_host/eseq/process_natives.rs`, `def-process`) every
builtin uses — so inlet storage, cable wiring, scene-scoped settings,
serialization, undo, the per-fire RNG seed (`fire_seed`) and the effective
value readouts all work unchanged.

- The project stores the **source text** on the slot, not the hash. On load
  the host recompiles every expr source it finds; identical sources share one
  class.
- Hashed classes never appear in the add-process dropdown or in
  `graph-node-process-classes`; the dropdown shows `expr` and the presets (§6).
- The bay label for an `expr#…` slot is `expr`.
- Unused hashed classes may be dropped from the registry when nothing
  references them; correctness does not depend on it.

As shipped (eseq-waa9.10): the class is built by handing a `def-process`
argument list (`:in`, `:targets ((out :mappable) (wire :process-inlet))`,
`:run (__expr-send! body)`) to the same `parse_process_def` every
`def-process` goes through, but the compiled definitions are held on
`SequencerState` (`register_expr_process_def`) and merged into every
`published_process_authoring()` read, not stored in the UI VM's authoring
registry. That registry is project-scoped (reset on a project switch) and
rebuilt with the UI runtime, while an expr class is a pure function of a
body the project stores; the graph natives also only hold the state. The
scheduler sees the classes through the same published snapshot. The hash is
48 bits of FNV over the whitespace/comment-normalized body
(`expr#` + 12 hex digits). The slot stores the body in
`TrackProcessSlot.expr_source`; project load re-derives each expr slot's
class name from its body (`project.rs`) and `finish_project_load` recompiles
every body in every scene and rack clip (`sync_expr_process_classes`). The
node chain reads also compile any expr slot whose class is missing (a kit or
instance copy). Node patch edits are not recorded in undo history (none of
the `graph-node-process-*` edits are), so neither is a commit.

## 3. Inlets come from the code

There is no inlet syntax. After parsing, every symbol in **argument position**
that is not

- a native or global function/value visible to process bodies,
- a `$` context variable (§4),
- a name bound inside the body (`let`, `state` (§5), function parameters),

becomes an inlet named after the symbol, in order of first appearance.

```lisp
(sin (* x rate))
```

gives two inlets, `x` and `rate`. Cable `acc` into `x`; leave `rate` as a
number picker. There is no `$1`: on a node every inlet is already a scalar
you can set by hand until a cable lands on it, so a named free symbol is both
the "input" and the "knob".

Rules:

- An unknown symbol in **function position** (`(sinn x)`) is a compile error,
  not an inlet.
- A typo in argument position (`(* x rtae)`) becomes a visible new inlet. That
  is acceptable feedback; the reconciliation toast also shows it.
- Inlet names share a namespace with nothing else in the body: the compiler
  rewrites each inlet symbol to `(in :<name>)`, so an inlet may not shadow a
  native, and a native added later that collides with an existing inlet name
  is resolved in favour of the inlet for that slot until the body is
  recommitted (recommit reports the change). The existing gotcha — a local
  named like a native (`neuron`) breaks the call — is rejected at commit with
  a clear error instead of an `ExpectedFunction` at run time.
- Inlets are declared `:float` with `:lane true`, so on a track each inlet is
  a lane like any other lane-capable inlet; on a node it is a scalar.

As shipped (eseq-waa9.10): "visible to process bodies" is the set of globals
and macros of a scheduler scratch runtime loaded with the MIDI-fx and process
libraries (built once per process), plus the special forms `if do let lambda
and or quote set!` (not `fn`: eseqlisp has no `fn` lambda, so `(fn …)` is a
commit error pointing at `lambda`) and the opcode heads (`+ - * / < <= = > >= list max min
nth len`). Inlets are declared `:float -1e6 1e6 :default 0 :lane true`; the
range only bounds pickers, it never clamps a cable. Inlet symbols are bound by
def-process's standard inlet binding (each inlet is a parameter bound to
`(in :name)`), which is equivalent to the rewrite. `$` names are never inlets;
a `$` name outside `EXPR_CONTEXT_VARS` (§4, eseq-waa9.12) is a compile error.
Also rejected at commit: `set!` on a name the body did not bind, heads
starting with `def`, quasiquote, and a local (let / lambda parameter) named
like a native, special form or `true`/`false`/`nil`, destructuring let /
lambda patterns and a bare-symbol lambda parameter list (the VM binds plain
symbols only), and an inlet named `&` or `&rest`. Kit loads recompile the
bodies they bring in (`sync_expr_process_classes`), like project load.

As shipped (eseq-waa9.12), the shadowing rule replaces "may not shadow a
native": scheduler globals such as `note vel dur step pan out in on tap get
mod mix str box ps` used to be silently non-inlets, so `(* vel 2)` committed
and then failed every fire. Now a symbol in argument position becomes an
inlet unless it is a literal, a local, a `$` name, a special form or opcode
head (the compiler lowers those whatever is in scope), or a global whose
value is not callable (a constant is read as is). A name that is otherwise
a global function or macro therefore becomes an inlet that shadows the
global for this body (`ExprAnalysis.shadowing` lists them). If the same body
also calls that name in function position, the commit fails on the argument
occurrence: "`vel` is used both as an inlet and as a function — rename the
inlet". `in` is an ordinary shadowing inlet, so the preset body `(* in k)`
gives inlets `in`, `k`: the run lambda's arguments `(in :in) (in :k)` are
evaluated outside the lambda, where `in` is still the global (pinned by a
unit test on the generated source and an end-to-end run). The names the
compiled body calls itself (`target-add!`, `target-set!`, `graph-reset!`,
`veto!`, `reset-fired?`, anything starting `__`) and the direct-write verbs
can be neither inlets nor locals. Passing a native as a value (`(map sin xs)`)
now makes an inlet named `sin`; wrap it in a lambda. A macro whose expansion
calls a name the body shadows is not detected.

As shipped (eseq-waa9.13), locals are relaxed the same way: a `let` name,
lambda parameter or `state` name may shadow a global function, macro or
stateful helper (`(let ((vel 2)) (* vel 3))`, `((lambda (count) …) 2)`)
unless the body also calls that name in function position anywhere — local
or global — which fails the commit on the binding: "`vel` is bound in the
body and also called as a function — rename the local". Special forms,
opcode heads, `true`/`false`/`nil`, reserved names and `$` names stay
unbindable.

### 3.1 Picker ranges

Parsed symbols carry no range. The default picker is an unbounded drag number
picker (fine step, shift for coarse). The range can be set per slot by
right-clicking the picker; it is stored with the slot, not in the source, so
the code stays free of widget syntax. (The Strudel-style `(~knob …)` forms of
`docs/inline-code-widgets-spec.md` are a code-buffer feature and are **not**
used here.)

As shipped (eseq-waa9.11): the default picker only. An expr inlet in the
selected-slot inspector is a `number-picker` with the new `:drag :relative`
mode: a drag moves `:drag-step` (0.1) per row from where it began, Shift
moves ten times that (switching mid-drag re-anchors), values snap to 0.01
from zero, and a missing `:min`/`:max` is open (the declared ±1e6 is not
passed). The per-slot range set by right-click is not built: follow-up
bead eseq-waa9.19.

### 3.2 Port overflow

The uniform card fits two or three ports. Open question: when a body has more
inlets than fit, either (a) the extra inlets live in the selected-slot
inspector only — settable there, not cable targets from the card — or (b)
commit fails past a cap. Default proposal: (a), with a `+n` badge on the card.

As shipped (eseq-waa9.11): (a), for every card in the bay, not only expr.
A card shows up to three in ports; with more it shows two and a `+n` badge.
Ports a cable lands on always show (so no cable loses its end), and the
remaining room goes to the first unwired inlets in order.

## 4. Context variables

Symbols starting with `$` are read-only context the host provides on every
run. They never become inlets.

| name | value |
| --- | --- |
| `$n` | fires of this node (steps of this track) since its last reset, 0-based |
| `$prev` | this card's last `wire` output (0 before the first) |
| `$note` | payload note, semitones, after earlier slots' writes |
| `$vel` | payload velocity, 0..1 |
| `$dur` | payload duration, beats |
| `$delay` | payload propagation delay offset, steps (nodes; 0 on tracks) |
| `$beat` | transport position, beats (`now-beats`) |
| `$phase` | position in the current bar, 0..1 |
| `$reset` | 1 on the first fire after a reset, else 0 (`reset-fired?`) |

`$note`/`$vel`/`$dur`/`$delay` read what the chain has written so far, the
same view `current-note` gives today.

As shipped (eseq-waa9.12): the compiler rewrites each read, so a fire pays
only for the names the body uses. `$note $vel $dur $delay $beat $phase
$reset` become `(__expr-ctx <index>)`, one native call that reads the step
context the chain runner rebuilds per slot (`$note`/`$vel`/`$dur` are the
payload's transpose / velocity / duration as the earlier slots left it,
including their mapped `out` writes; `$delay` is the new
`ProcessStepEventContext.delay_offset_steps`, the propagation-delay change the
earlier slots wrote through `(step-param :delay)`, 0 on tracks). All values
are a snapshot at the card's start: the card's own writes land after its run.
`$beat` is `now-beats`; `$phase` is `fract($beat / 4)` (a 4/4 bar, like
`(bars n)`); `$reset` is 1 when `reset-fired?` is true, else 0 — including a
graph's first fire after play starts, which counts as after a reset.
`$n` and `$prev` are hidden `:state` cells of the class (`$n` initial -1,
`$prev` 0), declared only when the body reads them. The run body advances
them at the run lambda's top level, where `set!` reaches the cell:
`(set! $n (if (reset-fired?) 0 (+ $n 1)))`, `(set! $prev (if (reset-fired?) 0
$prev))`, then `(set! $prev (__expr-send! body $prev))` (the send returns the
value sent, or `$prev` when the body gave nil). So `$n` is 0 on the first
fire and on the first fire after each reset, and `$prev` is the last value
sent (bools as 1/0), zeroed on reset. Like every def-process state cell they
also return to their initial values on stop → play
(`reset_step_process_states`). A failed run (bypass) stores no state, so it
does not advance `$n`. On a track `$n` counts the card's runs, one per step
the lane patch runs on (§12 Q2: steps, not trigs); tracks have no graph
reset, so only stop → play restarts it (expr on tracks lands with
eseq-waa9.18). `$` names are read-only: `set!` on one and binding one are
commit errors, an unknown `$` name is a commit error, and quoted `$` names
are plain data. The expr edit buffer's completion offers every `$` name and
direct write with its one-line doc (`expr-context-completions`, fed to the
new general editor native `(mode-add-completions mode items)`).

## 5. State

A process that remembers something across fires declares it:

```lisp
(state s 0xACE1)
```

`state` binds `s` for the body; its initial value is the second form. Any
number of `state` forms may appear. They compile to the class's `:state`
cells.

Requirements (these differ from `def-process` bodies today):

- `set!` on a state name works **at any depth** — inside `let`, `if`, nested
  `do`. Today a `set!` inside a nested `let` does not reach the state cell
  (see the comment above `lane-acc` in `builtin.lisp`); the expr compiler must
  rewrite state writes so this cannot happen, and a test pins it.
- State is independent of the output. An RNG keeps a 16-bit register and
  emits three of its bits; `$prev` is only the last output.
- On a reset, state cells return to their initial values before the body runs,
  unless the body handles `$reset` itself. A short-period LFSR therefore loops
  from the same point on every bar or graph reset.

### 5.1 Stateful helpers

Helpers that each own a hidden state cell per call site:

| form | value |
| --- | --- |
| `(prev x)` | `x` from the previous fire |
| `(delta x)` | `x - (prev x)` |
| `(integ x)` | running sum of `x` |
| `(sh gate x)` | sample `x` when `gate > 0.5`, else hold |
| `(slew x amt)` | one-pole toward `x` |
| `(every k x)` | `x` on every `k`-th fire, else nothing written |
| `(count k)` | fire counter wrapping at `k` |

Each call site gets its own cell, so `(prev x)` twice means two histories.

As shipped (eseq-waa9.13):

- **Form.** `(state name init)` (init optional, default 0) is allowed only at
  the top level of the body, in any position and any number; a `state` form
  anywhere else is a commit error. The forms are collected before the walk,
  so the name is bound for the whole body wherever its form sits, and they
  are dropped from the evaluated body (a body of state forms only sends
  nothing). `init` must be a number literal (decimal, hex or binary, signed);
  anything else — even `(+ 1 2)` — is a commit error. Commit errors, all
  span-anchored: a duplicate state name; a `$` name, special form, opcode
  head, literal, reserved or `__` name; a `let` local or lambda parameter
  that rebinds a state name; a state name that is also called as a function
  (§3 as shipped .13). A state name is bound, so it can never be an inlet.
- **`set!` at any depth.** State cells are NOT run-lambda parameters (the
  def-process wrapper would store the parameter back at the end and clobber
  a nested write). Every read of a state name lowers to `(__expr-cell 0
  :name)` and every `(set! name v)` to `(__expr-cell 1 :name v)`, anywhere:
  inside `let`, `if`, `do`, and lambdas (lambdas run inside the fire, so a
  write there is fine). `__expr-cell` (process_natives.rs) reads or updates
  the invocation's state map in place, borrowing the key — no allocation
  beyond the per-run state map clone every process run already makes. The
  cells are appended to the class's `ProcessDef.state` after the run body is
  wrapped, so the runtime still initializes them, and stop → play
  (`reset_step_process_states`) clears them like `$n`/`$prev`. A failed run
  (bypass) stores none of its writes. Two cards with the same body share the
  class but not the cells (state is keyed by slot instance).
- **Reset.** The run body starts with `(if (reset-fired?) (do (__expr-cell 1
  :s init) …) nil)` over every state and helper cell, so on a node's first
  fire after a reset (and on the graph's first fire) every cell is back at
  its init before the body runs. Deviation: this is unconditional — reading
  `$reset` does not opt out. A body can still read `$reset` and set its
  state after the automatic restore; it cannot carry state through a reset.
- **Helpers.** `(prev x)`, `(delta x)`, `(integ x)`, `(sh gate x)`, `(slew x
  amt)`, `(every k x)`, `(count k)` are rewritten per call site to
  `(__expr-cell <op> :__h<n>-<helper> args…)`, the hidden cell numbered in
  source pre-order (initial 0) and declared after the user state. Semantics:
  `prev` returns the previous fire's x (0 first); `delta` is x minus that;
  `integ` returns the running sum including this fire; `sh` stores x when
  gate > 0.5 (a bool gate counts as 1/0) and returns the held value (0 until
  the first sample); `slew` is `y += amt * (x - y)` with amt clamped to 0..1
  and y starting at 0; `count` returns 0, 1, … k-1, 0, … (k floored, below 1
  counts as 1; a live change of k wraps the current count); `every` returns
  x on the fire where that counter is 0 — the first fire and every k-th after
  it — and nil (nothing sent) otherwise. The numeric helpers fail the run
  (bypass) on a non-number. All helper cells reset with the state cells.
  Arity is checked at commit. A helper call inside a lambda is a commit
  error ("keeps one history per place it is written"): one lexical site
  called many times would share one cell. Not detected: a helper inside a
  looping or lambda-building macro's arguments. (A helper written through
  `->` works since eseq-waa9.15: threading expands before analysis.) The names win in function position over the scheduler
  globals `count` / `every`; used as an argument they are ordinary
  (shadowing) inlets, and a body that both calls a helper and uses its name
  as an inlet is the usual inlet/function clash.
- **Recommit.** The class hash covers the normalized body, and the rewrite
  is a pure function of it (cell names from source order), so identical
  bodies give identical run sources and cells. A recommit that keeps a cell
  name keeps its running value (the runtime reconciles cells by name, as for
  any def-process); helper cell names carry the helper so a changed helper
  kind at the same site does not inherit a foreign value. This mirrors the
  inlet reconcile rule (§2: keep by name): a recommit that only changes a
  cell's `init` keeps the running value, and the new init takes effect on the
  next reset or stop → play; a renamed cell starts at its init and the old
  one is dropped. Helper cells are numbered by source order, so inserting a
  helper call before an existing one of the same kind shifts which history
  each site inherits (the kinds guard against foreign values, not order).
  A `set!` without exactly one value (`(set! s)`, `(set! s 1 2)`) is a
  commit error rather than a run error on every fire.
- **Completion.** `expr-context-completions` also lists `state` (category
  "expr state") and the seven helpers ("expr helper") with one-line docs.
- **For promote (§8, eseq-waa9.17).** State and helper cells map to
  `:state` entries, but a def-process body cannot reach them at depth with
  plain `set!`: the promoted `:run` must keep the `__expr-cell` rewrite (with
  the cells declared so the wrapper does not bind them as parameters) or the
  translation must refuse bodies whose state writes are not at the top level.

## 6. Outputs and direct writes

The value of the body is sent on the `wire` port (as `acc` and `rand` do) and
can also be mapped to a payload target with the existing map chips. A body
that evaluates to `nil` sends nothing that fire.

Bodies may also write the payload directly:

| form | effect |
| --- | --- |
| `(veto!)` | mute this emission (it still scatters, as today) |
| `(delay! x)` | add `x` steps to the propagation delay (node) |
| `(xpose! x)` | add `x` semitones |
| `(vel! x)` | set velocity (0..1) |
| `(dur! x)` | set duration (beats) |
| `(reset! group)` | request a graph reset after this fire (`graph-reset!`) |

They are thin wrappers over the existing verbs (`veto!`, `target-add!` on the
step-param pseudo targets, `graph-reset!`). The compiler collects which ones
a body uses and declares the matching `:targets` on the hashed class. So one
card can do "every fifth fire push the delay and duck the velocity" without
three cards and two cables.

As shipped (eseq-waa9.12): each write lowers at compile time to its verb
followed by `nil`, so every write evaluates to nil and a body that only
writes sends nothing. `(veto!)` → `(veto!)`; `(delay! x)` → `(target-add!
:delay! x)`; `(xpose! x)` → `(target-add! :xpose! x)`; `(vel! x)` →
`(target-set! :vel! x)` (the step-param clamp keeps it in 0..1); `(dur! x)`
→ `(target-set! :dur! x)` (the payload's duration field: beats on a node);
`(reset! [group])` → `(graph-reset! group)`, the group as `graph-reset!`
takes it (`:all`/0/omitted, 1 = A, or a letter). The class declares one
fixed step-param port per write the body uses, after `out` and `wire`, in
the order delay!, xpose!, vel!, dur! (`(delay! (step-param :delay))` …);
`veto!` and `reset!` are commands and need none. The ports are not
connectable, so the card shows no extra ports. The write set is part of the
body, hence of the hash. Writes may appear anywhere (inside `if`, `let`,
`do`, lambdas). Arity is checked at commit. `(vel! event v)` and `(dur!
event v)` with two arguments stay the ratchet-shape event mutators. On a
track, `delay!` has no effect (as with `neural-delay`), and `reset!` asks
for a graph reset that tracks do not honour.

### 6.1 Presets

The add-process dropdown gains rows that are just expr cards with a body filled
in: `× k` = `(* in k)`, `+ k` = `(+ in k)`, `sin` = `(sin in)`, `quant` =
`(quant in step)`, `fold` = `(fold in lo hi)`, `wrap`, `scale`. The basic math
blocks come from one mechanism instead of a dozen classes, and every preset
can be opened and edited.

Rhythm-shaped helpers available to every body: `(quant x step)`,
`(fold x lo hi)`, `(wrap x lo hi)`, `(scale x in-lo in-hi out-lo out-hi)`,
`(euclid k n i)`, `(choose a b …)` (uses the per-fire RNG), plus the
`alez.sig` shapers `sine`/`tri`/`saw`/`sqr` on a 0..1 phase and the `->`
threading form, so `(-> x (* rate) sin (scale -1 1 0 4))` reads in chain order.

As shipped (eseq-waa9.15, node bays):

- **Threading.** `->` / `->>` are expanded on the spanned AST before
  analysis (`expand_threading`, the compiler's own semantics: a bare symbol
  stage `f` is `(f acc)`, a list stage gets acc inserted first / last, the
  result expanded again; quoted data untouched), so helpers written through
  a threaded form get their arity check and cells (`(-> x prev)`,
  `(-> x (slew 0.5) prev)`) and inlets are derived from the expanded form.
  The class hash stays on the authored body. Expansion adds one nesting
  level per stage, so the expanded depth is held to `EXPR_MAX_NESTING` too.
  `(->)` and a stage that is neither a symbol nor a call are commit errors.
- **Constants.** `pi`, `tau` lower to number literals; never inlets, not
  bindable (`let`/lambda/`state`), not callable.
- **Shaping helpers** are pure and lower to one internal native call,
  `(__expr-fn <op> args…)` (`choose` → `(__expr-choose args…)`), registered
  on the scheduler VM in `process_natives.rs`. No user-visible scheduler
  global was added: `quant fold scale euclid choose sine tri saw sqr unipolar
  bipolar` exist only inside expr bodies. `wrap` and `clip` were already
  scheduler globals (same semantics); in function position inside a body the
  expr helper wins, as for `count`/`every`, used as an argument they stay
  ordinary (shadowing) inlets. (`bounce`, the existing global fold, is
  untouched.) Semantics (`expr_pure_fn`): `(quant x step)` nearest multiple,
  halves away from 0 (`f64::round`), step sign ignored, step 0 passes x, no
  `-0`; `(fold x lo hi)` ping-pong, both edges reachable; `(wrap x lo hi)`
  half-open (hi → lo); `(scale x in-lo in-hi out-lo out-hi)` linear, not
  clamped, an empty input range gives out-lo; `(clip x lo hi)`; swapped
  bounds are reordered and an empty lo..hi gives lo for fold/wrap;
  `(euclid k n i)` is 1 when `(i·k) mod n < k` (Bresenham form, hits on step
  0: `(euclid 3 8 i)` = `x..x..x.`; `(euclid 5 8 i)` = `x.x.xx.x`, a
  rotation of Bjorklund's `x.xx.xx.`), k and n floored, k clamped to 0..n,
  i wraps mod n (negatives too), n < 1 never hits; `(choose a b …)` picks
  one argument (any value) uniformly from the per-fire process RNG stream
  `rand` advances (reproducible per fire seed); shapers of a phase as in
  `alez.sig`: `(sine p)` = 0.5 + 0.5 sin(τp), `(tri p)` 0 at 0 / 1 at 0.5,
  `(saw p)` = p mod 1, `(sqr p [duty 0.5])` 1 while p mod 1 < duty;
  `(unipolar x)` = 0.5 + 0.5x, `(bipolar x)` = 2x − 1. A non-number
  argument fails the run (bypass); bools count as 1/0; NaN in any argument
  gives NaN (euclid/sqr included); `rem_euclid` results are kept half-open.
  Arity is checked at commit; a body binding a helper name and calling it
  is the usual clash error.
- **Completion** (`expr-context-completions`) adds the shapers ("expr
  shaper"), `pi`/`tau` ("expr constant") and `->`/`->>` ("expr form").
- **Presets** are one Lisp table, `expr-presets` in
  `content/ui/expr-buffer.lisp` (`preset-list`, `preset-labels`,
  `preset-named`, `add-preset label source inlets` to extend or replace by
  label, `add-node-preset graph node preset`). Rows, with the starting inlet
  values the pick sets: `× k` `(* in k)` k=1; `+ k` `(+ in k)` k=1; `sin`
  `(sin in)`; `quant` `(quant in step)` step=1; `fold` / `wrap` `(fold|wrap
  in lo hi)` lo=0 hi=1; `scale 0..1 → lo..hi` `(scale in 0 1 lo hi)` lo=0
  hi=1 (not labelled `scale`: the node menu already has `scale`, the
  musical-scale class `neural-scale`, and a pick arrives as its label);
  `bounce` `(* k (pow decay $n))` k=1 decay=0.8; `lfsr` the §9 body
  (multi-line) taps=46080 (0xB400) grain=1. The input inlet is `in` (an
  ordinary shadowing inlet, §3 as shipped .12), left at 0 until cabled.
  The node bay's add menu (`gvr-proc-add-card`, via the new
  `eseq.sequencer/lane-patch-add-menu-grouped`, which passes `:headers`)
  lists the classes, then a dim `expr presets` heading, then the preset
  rows; a pick adds an `expr` card, commits the body through
  `eseq.expr-buffer/commit-source` (error dot / toast bookkeeping) and sets
  the inlets with `graph-node-process-inlet`. Track bays keep their plain
  lane menu until track expr cards land (eseq-waa9.18). Capture fixture:
  `crates/sequencer/ui/capture-fixtures/graph-node-expr-presets.lisp`.

## 7. The `*processes*` dock

An inspector for the process card selected in an open node editor, over
that card's expr code when it is an expr card. Adding processes is the
bay's add-process dropdown; the dock does not repeat it.

History: eseq-waa9.16 shipped the dock as a process library (groups by
source — Builtin / packages / My processes / Expr presets / Scripts —
search, favourites, a detail box, add by `+` or drag onto the bay) stacked
under the browser. User feedback (2026-09-27) found it duplicated the
dropdown and crowded the sidebar; eseq-waa9.22 replaced it with the
inspector below and removed the library UI and its host support.

As shipped (eseq-waa9.22, node editors): `content/ui/processes-buffer.lisp`
(`eseq.processes-buffer`), the `*processes*` effect buffer.

- **When.** `showing?` = defcustom `processes-buffer-auto-split` (default
  true) and a node editor open in the main tile: the visible main-panel
  buffer is an instance step tab whose instance registered a node bay
  (`eseq.sequencer/lane-patch-node-host?`) and whose `expanded-node` view
  field is a node (`dock-target` = `(instance node)`). Track lane patchbays
  do not open it yet (their expr cards are eseq-waa9.18).
- **Layout.** While showing, the dock takes the right column of the
  regular layout (`eseq.seq-layout/step-and-track-panel-layout-spec`): the
  inspector in place of `*step*`, the selected expr card's code in place of
  `*track*`, split by defcustom `processes-code-ratio` (0.5; the code
  tile's min-height floor is 3 so the ratio wins). Without code the
  inspector fills the column. The dock tiles keep the column's width (28
  cells), so the main tile does not move. The browser sidebar (and its
  `*groove*` split) is never touched. The column is chosen from the
  visible main-panel buffer, `eseq.seq-step-tabs/step-panel-buffer`: the
  Seq tab (or any tab without an open node editor) gets `*step*`/`*track*`,
  the instance tab the dock, with its node, selection and code state
  (per instance / defstates, untouched by the switch). A mouse click on
  the main tile's tab strip swaps the tile's buffer inside the editor, so
  every main step tab (Seq included) carries an eseqlisp tab `:on-select`
  (`seq-main-step-tab-selected`) that moves `step-panel-buffer` to the
  clicked tab. One observer re-lays out when the dock appears or goes, its
  node changes, or its code tile changes (key: instance, node, code slot),
  in the `:lower-panel` layout only. History: first shipped as the whole
  left sidebar (hiding the browser); moved to the right column on user
  feedback 2026-09-27.
- **Inspector.** Header: `*step*`'s header pill (`eseq.panel-header`,
  shared with `*step*`'s track badge row): the instance label in an accent
  chip, then `node N · <card label>` muted; **show code** / **hide code**
  at the right on an expr card. Under it the selected card (the bay's
  selection, `lane-patch-node-selected`, while that slot is in the node's
  chain): label, on/off, `<` `>` `x`, the inlet pickers (same relative
  pickers and values as before; a cabled inlet reads `wired`), each
  mappable port's `out -> … map` row (the neuron row's pickers turn into
  target chips while armed), promote… on an expr card with a body, as expr
  on others (live on a promoted class, dim with the reason otherwise), and
  lane-harmony's snap meter. Nothing selected: the hint "select a process
  card". Node-level controls (transpose, velocity, …) stay in the expanded
  editor.
- **Inspector seam.** `eseq.processes-buffer` is core UI and cannot import a
  package, so a kind that hosts node editors registers its card renderer,
  keyed by its instance kind, from the event that expands a node (next to
  `lane-patch-register-node`): `(eseq.processes-buffer/register-node-inspector
  self.kind renderer)`, `renderer` = `(fn graph node slot-id) -> widget`.
  The registry is a `defstate`, so re-evaluating processes-buffer.lisp
  (C-c C-c, eval buffer, hot reload) keeps it; the kind registers a
  trampoline `(lambda (g n s) (gvr-proc-inspector g n s))`, so
  re-evaluating the kind's file restyles the dock at once (a stored
  function value would keep drawing the old definition).
  `alez.neural.variable-reset` registers `gvr-proc-inspector`, which draws
  `gvr-proc-card` at full width with promote's modal mounted in the dock
  (`origin` "dock"). The bay draws the same `gvr-proc-card` (width 15,
  origin "node") beside the patchbay only when `(docks? graph node)` is
  false — dock setting off, another layout (patcher) — so the two
  homes cannot drift.
- **Code.** The code tile is the real `*expr node N · slot M*` text buffer
  (`eseq.expr-buffer/ensure-node-slot-buffer` creates it without switching;
  a set-layout tile like the instrument patcher's source pane). Selecting
  an expr card shows it: code is on by default (an expr card is its code,
  and the user wanted it one click away); **hide code** gives the inspector
  the whole dock and is remembered for the session (`code-visible`
  defstate, not persisted). A class card never has a code tile. The card's
  edit button, while docked, turns code on, lays out and focuses the tile
  (`open-docked`, via the editor native `(select-window-for name)`, a tile
  op applied after the queued set-layout). Promote or removal (the node
  patch version bump) drops the tile. Mode, completion, C-c C-c / save are
  unchanged. With the dock off the edit button opens a normal buffer.
- **My processes** (§8) is the user-tier package `user/processes`
  (`MY_PROCESSES_PACKAGE`, module / id prefix `user.processes`,
  `pkg:user.processes/…`) at `<user_lisp_root>/packages/user.processes/`
  (`my_processes_package_dir()`, `~/.eseq.d/packages/user.processes/`),
  `def-process` modules under `src/`; `process_library.rs` keeps only
  these helpers. Promoted classes appear in the add-process dropdown like
  any other.
- **Removed with the library (eseq-waa9.22):** the `graph-node-process-library`
  native and its source classification (`ProcessClassSource`,
  `process_class_sources`, `process_class_inlets_value`); `SEQ.process-library`'s
  `:source :package :inlets` fields; the favourites store
  (`process_favorites.rs`, natives `process-favorites` /
  `process-favorite-toggle`; an existing `process-favorites.json` is left
  on disk, unread); the bay's `process-class` drop target; defcustoms
  `processes-buffer-ratio` / `processes-source-ratio`.
- Capture fixture `crates/sequencer/ui/capture-fixtures/processes-dock.lisp`
  (`--buffer "*processes*" --all-panels`): an expr card selected, inspector
  over code.

## 8. Promote to library

A **promote** action on an expr card (card menu and dock) writes a real
`def-process` into the user's own package (**My processes**, under the package
tier: `pkg:<author>.processes`, `src/`), exported, with a name the player
picks. The translation is mechanical:

| expr | def-process |
| --- | --- |
| free-symbol inlet + its per-slot range | `:in` entry, `:float`, range, default = current value, `:lane true` |
| `state` forms and helper cells | `:state` entries |
| direct writes used | `:targets` entries |
| `wire` output | `(wire :process-inlet)` target plus `(out :mappable)` |
| body | `:run`, with inlet symbols rewritten to `(in :name)` |
| a `;` comment on the first line | `:doc` |

The live card is then rebound to the new class in place, keeping its inlet
values and cables. The class appears in the add-process dropdown for every
node and track and can be shared with Export Package.

The reverse, **edit as expr**, opens a library class's `:run` body as a new
expr card in place of the slot, when the body uses only what expr can express
(otherwise the action is disabled with the reason).

As shipped (eseq-waa9.17, node bays):

- **File format: `def-process … :expr "<body>"`, not lowered code.**
  `def-process` gained an `:expr` option: the class is compiled from the
  body by the same expr pipeline a card uses (`expr_process::expr_process_def`,
  shared by `compile_expr_source`; `parse_process_def` delegates when it sees
  `:expr`). Inlets, `state`/helper cells (still appended after the wrapper,
  so `set!` at any depth keeps working), `$` variables, write ports and the
  run body all come from the body, so a promoted class cannot drift from
  the card and the file never exposes `__expr-*` internals. Beside `:expr`
  only `:doc` and `:in` are allowed; an `:in` entry replaces the derived
  inlet of that name (range, default, kind, lane, doc) and must name one.
  `ProcessDef`/`PublishedProcessDef` carry `expr_source`, which gives the
  class the expr step budget (§10) and makes it reopenable as expr.
  Example (`<user lisp root>/packages/user.processes/src/bouncy.lisp`):

  ```lisp
  ;; My processes: `bouncy`, promoted from an expr card …
  (module user.processes.bouncy)
  (export bouncy)

  (def-process bouncy
    :doc "promoted from an expr card: (* k (pow decay $n))"
    :in ((k :float -1000000 1000000 :default 1 :lane true)
         (decay :float -1000000 1000000 :default 0.8 :lane true))
    :expr "(* k (pow decay $n))")
  ```

  One module per process (`user.processes.<name>`, which the package
  validator requires of every `src/` file); the class registers as
  `user.processes.<name>/<name>` and the bay, add menu and dock label it
  `<name>` (`graph_node_process_label` drops a module qualifier). `:doc` is
  the body's first line when it is a `;` comment, else "promoted from an
  expr card: <body>". Defaults are the card's current values; ranges are
  the expr default ±1e6 (per-slot ranges are eseq-waa9.19). The first
  promote creates the package: `manifest.json` (`user/processes`, no
  entry) + `src/`. A body containing `"` (a string literal, or quotes in
  a comment) is written as `:expr (str "…" (string-from-char-code 34)
  "…")` — eseqlisp strings have no escapes and `:expr` is evaluated at
  load, so the body round-trips exactly; `:doc` turns `"` into `'`.
- **Promote.** Natives: `(graph-node-process-promote-check seq name)`,
  `(graph-node-process-promote seq node slot name)` (validates, compiles
  the would-be class in memory, writes via temp + rename, refuses an
  existing file; does not load or rebind), `(graph-node-process-rebind-class
  seq node slot class)`. Name rule: `[a-z][a-z0-9-]*`, ≤ 40 chars, no
  trailing/doubled `-`, not `expr`/`expr-…`, not the name, unqualified name
  or label of any existing class — except the user's own My processes
  class of that name (`user.processes.<name>/<name>` defined by a file in
  the package, or a file there whose class did not load), which makes the
  promote an **update**: the check answers `:update true`, the modal says
  so and its button reads **Update**, and the write needs the native's
  `replace` argument (otherwise refused "confirm to update it"); the file
  is replaced atomically (temp + rename), reloaded (upsert: one class, not
  two) and the card rebound. Every other card of the class, in any node
  or scene, runs the new definition at once: inlets resolve by name, a
  new inlet takes the file's default, a removed one's stored value and
  any cable into it are left inert (not pruned). Builtin and other
  packages' classes are never replaced. "As expr" from a My processes
  class remembers the name (native `:origin`, session memory in the UI),
  so promote on that card opens prefilled with it: as expr → tweak →
  promote updates in place. `eseq.expr-buffer/promote-node-slot`
  writes, `load`s the module into the UI VM (hot) and rebinds; the name
  modal (`open-promote` / `promote-panel`, mounted by the node editor and
  by the dock, `promote-origin` picks one) opens from a **promote…** button
  on the selected-slot inspector of an expr card (in the dock, or inline
  in the bay when the dock is off; eseq-waa9.22 dropped the dock-header
  button). Bay cards have no context menu, so there is no card-menu entry.
- **Rebind.** Same slot id and position, outgoing bindings and fan-out;
  inlets and incoming cables kept by name (all survive: same names); the
  slot's `expr_source` is dropped, so it is an ordinary card (no edit
  button, class name as label). Runtime state is keyed by slot instance and
  reconciled by cell name; the promoted class has the same cells, so state
  carries (pinned by a lowering-equivalence test).
- **Edit as expr.** `(graph-node-process-edit-as-expr seq node slot)`: a
  class with an expr body (a `:expr` def-process) becomes an expr card
  holding that body; every inlet's current value, class defaults
  included, is written onto the slot first (an expr card defaults to 0), so
  it sends what the class did. Other classes refuse with a reason. The
  inspector shows **as expr** on every non-expr card: live on a promoted
  one, dim on others where a click puts the reason on the status line. The
  chain read gains `:promoted-expr` and `:as-expr-reason`. Only `:expr`
  classes are reopened; hand-written `:run` bodies are not translated.
- **Persistence.** The UI runtime loads every `src/**/*.lisp` of the
  package right after builtin.lisp (`load_my_processes_source`), and
  `register_process_def` marks any def whose source file lies in the
  package as a package-layer def, so promoted classes survive project
  switches (startup and hot loads alike). A project stores the slot's class
  name; with the package missing the slot keeps it and does nothing, the
  chain read shows `:known false` — the same as any process whose package
  is gone.
- **Dock.** (eseq-waa9.17 grouped promoted classes under the library's My
  processes; the library is gone since eseq-waa9.22.) Tests:
  `set_my_processes_package_dir_override` points the package at a temp dir
  (never the real `~/.eseq.d`); the override is thread-local and restored
  when its guard drops.
- **Sharing.** The package dir is a valid installed package
  (`InstalledPackage::load`), and its modules show in the Packages tab
  after the catalog refresh; but Export Package… exports instruments,
  effects and presets only, so a My processes class is shared by copying
  or zipping `packages/user.processes/` for now.

## 9. Bitwise math

Classic shift-register sequencers — xorshift, Galois LFSRs, Turing-Machine
style rotate-and-flip — need integer bit operations. eseqlisp numbers are all
`f64` (`Value::Number`, `lang/vm.rs`), and there are no bitwise natives.

- New natives, available to every process body (and the rest of eseqlisp):
  `bit-and`, `bit-or`, `bit-xor`, `bit-not`, `shl`, `shr`. Arguments are
  truncated to unsigned 32-bit, results wrap to 32 bits and return as numbers.
  Every 32-bit word is exact in an `f64`. As shipped (eseq-waa9.14,
  `register_math_natives` in `lang/vm.rs`): each argument is truncated toward
  zero and wrapped modulo 2^32, so `-1` is `0xFFFFFFFF` and `2^32 + 3` is `3`;
  `bit-and`/`bit-or`/`bit-xor` fold over two or more arguments; `shl` drops
  bits past 32 and `shr` is a logical (zero-filling) shift, both masking the
  shift amount to 0..31. Like the other math natives, a missing, non-numeric
  or non-finite argument (or fewer than two for the folds) returns NaN rather
  than raising.
- **Hex literals.** `0xACE1` must parse as a number. Today the tokenizer reads
  `0` as a number and `xACE1` as a separate symbol (`lang/parser.rs`,
  `parse_number`), which under §3 would silently become an inlet named
  `xACE1`. Hex (and ideally `0b` binary) literals are part of this slice.
  As shipped: `0x`/`0X` hex and `0b`/`0B` binary integer literals, with an
  optional sign (`-0x10`). The whole token up to the next delimiter must be
  valid, so `0x`, `0xZZ`, `0b102` and `0x1.5` are parse errors rather than a
  number followed by a symbol. The editor highlighter classifies them as
  numbers through the same `parser::parse_radix_integer_literal`.

Example — a 16-bit Galois LFSR driving delay:

```lisp
(state s 0xACE1)
(set! s (bit-xor (shr s 1)
                 (if (= (bit-and s 1) 1) taps 0)))
(delay! (* grain (bit-and s 7)))
```

`taps` and `grain` are inlets; changing `taps` live changes the period and
the character of the pattern, and a reset restarts it from `0xACE1`.

## 10. Safety

User code runs on every fire on the scheduler thread.

- Each run has an evaluation step budget; exceeding it is a runtime error
  (§2: bypass for that fire, error dot, message in the source buffer).
- Nesting depth is checked at commit. Compiler traversal is still recursive in
  authored nesting (`eseq-4tl.1`), so deep source must be rejected before it
  reaches the compiler.
- Only natives already allowed in process `:run` scope, plus the helpers in
  this spec, are callable; the commit step rejects anything else by name.

As shipped (eseq-waa9.10): the eseqlisp VM has an opcode step budget
(`VM::set_step_budget`, `VMError::StepBudgetExceeded`, shared by nested calls
natives make back into the VM). `ProcessRunInvocation.step_budget` carries it;
`expr#…` classes get `EXPR_PROCESS_STEP_BUDGET` = 20 000 opcodes per fire,
every other process runs unlimited as before. A failed run (budget or any
error) applies none of its commands, so the card is bypassed for that fire and
the next card still runs; `ProcessRuntime` keeps the last error per runtime id
and the lookahead mirrors it to `SequencerState::process_run_error`. Nesting
is checked on the flat token stream before the recursive parser runs:
`EXPR_MAX_NESTING` = 32 list levels, `EXPR_MAX_TOKENS` = 4096.

## 11. Slices

| bead | slice |
| --- | --- |
| eseq-waa9.10 | Expr core: hashed class from source, free-symbol inlet derivation, commit + reconcile + in-place rebind, source in project file, error bypass, step budget + depth check. Headless natives + tests. |
| eseq-waa9.11 | Expr card + edit buffer: uniform card with preview label and edit button, `*expr …*` buffer, commit key, error dot, per-slot picker ranges, port overflow (§3.2). |
| eseq-waa9.12 | Context variables (`$n`, `$prev`, `$note`, …) and direct writes (`veto!`, `delay!`, `xpose!`, `vel!`, `dur!`, `reset!`). |
| eseq-waa9.13 | `state` form with `set!` at any depth, reset-to-initial, stateful helpers (§5.1). |
| eseq-waa9.14 | Bitwise natives + hex/binary literals (§9). |
| eseq-waa9.15 | Presets and rhythm helpers in the add-process dropdown (§6.1). |
| eseq-waa9.16 | `*processes*` sidebar dock: library, search, favourites, hosts the expr source buffer (§7). |
| eseq-waa9.17 | Promote to My processes + edit as expr (§8). |

Order: .10 → .11 → (.12, .13, .14 in any order) → .15 → .16 → .17.
.14 has no dependency on the rest and can land first.

## 12. Open questions

1. ~~Port overflow on the uniform card (§3.2).~~ Resolved: (a), as shipped.
2. ~~Whether `$n` on a track counts steps or trigs.~~ Resolved (eseq-waa9.12):
   steps — the card's runs, as §4 as shipped says.
3. ~~Whether the dock can host a text-mode buffer as-is (§7).~~ Resolved
   (eseq-waa9.16): yes, as a layout tile; only a focus-by-buffer tile op
   (`select-window-for`) was added.
