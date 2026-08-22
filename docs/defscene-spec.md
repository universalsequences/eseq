# defscene Spec

A scene-varying storage class for eseqlisp values. `defscene` declares a named
slot whose value is stored **per pattern**, serialized with the project, read
as a bare symbol, and written with `set!`:

```lisp
(defscene jb-figures '((. . . .)))

(each jb-figures |f| (jb-figure-row f))                 ; reactive read
(set! jb-figures (append jb-figures (list new-fig)))    ; persists per scene
```

Scene 1 can hold `(. . -)` while scene 2 holds
`(fig (. -) (every 2 rev)) (fig (. . -))`; switching scenes just plays — and
displays — the other one. No republish, no sync functions, no shadow state.

## Motivation

Script-defined sequencers want per-scene configuration the way the graph
sequencer already has it. The graph demo
(`content/scripts/sequencers/graph-neural-variable-reset-demo.lisp`) works
across scenes because its *definition* is published once while its *data*
(node/edge/config overrides) lives in the pattern and is resolved at tick
time. Jaki-style DSLs have no equivalent store: their body is baked into the
published tick, so per-scene bodies would mean republishing (and recompiling)
on scene switch — reopening a scene-switch-latency fight already won twice.

The motivating consumer is a UI builder frontend for jaki ("build figures,
choose modifiers via dropdowns") whose figure state must be scene-locked. But
the primitive is general: any script UI gains per-scene configuration (a sig
whose `:over` differs per scene, builder panels for future DSLs, prepared-swap
live-set state).

## Design principle

**The symbol is the API.** The declaration carries all machinery; use sites
are ordinary Lisp. Reads are the bare name, writes are `set!` — identical to
`defstate` — and the storage class changes only lifetime and ownership. This
is the same move that made `defchan` trustworthy (late-bound names survive
re-eval) and `bind-graph` pleasant (no per-node shadow state). If a use site
ever needs to know it is touching a scene slot, the design has failed.

## Surface

```lisp
(defscene name default)
```

- `name` — flat symbol; inside a declared module it qualifies like `defstate`
  names do.
- `default` — the value a pattern with no stored override resolves to.
  Evaluated once at declaration.

Values are restricted to **portable literals** — numbers, strings, keywords,
bools, nil, and nested lists/dicts of those (the `ProcessLiteral` vocabulary,
already enforced for channel initials). Functions, widgets, and host handles
are an immediate authoring-time error naming the slot. This is what makes the
value serializable and shippable across the VM boundary.

Semantics:

- **Read** (bare symbol): resolve against the current pattern's slot store;
  fall back to the declaration default. In UI rendering context the read is
  reactive (see Seam 2).
- **Write** (`set!`): store the value into the *current* pattern's slot store
  and dirty exactly the widgets bound to this slot. Writes are override
  writes: they never touch other patterns or the default.
- **Re-evaluating the `defscene`** rebinds the default and value validation
  but does NOT clobber stored per-pattern overrides — the same rule as
  "loading does not write overrides" for graph scripts.
- **Removing a `defscene`** (buffer re-eval without it) orphans stored values
  harmlessly; they persist in patterns and reattach if the declaration
  returns. (Same tolerance channels have for late binding.)

## The three seams

The pleasant surface means the machinery lives in three places, all
extensions of existing seams:

### Seam 1: compiler registration and lowering

`defscene` registers its name in the same scope machinery `defstate` uses
(`compiler.rs`, the `defstate` special form is the template). A bare read of a
registered scene name compiles to a slot-resolve against the current pattern;
`set!` on one compiles to the persist-plus-dirty write. Declaration order
follows `defstate`'s existing rules (a read before the declaration in the same
unit behaves as `defstate` would).

### Seam 2: reactive dependency injection

Scene-switch repaint in the graph demo works because `bind-graph` reads
through `current-pattern`. Bare-symbol scene reads inject an equivalent
dependency invisibly, but qualified per slot: a read during widget rendering
registers `(__scene-slot, slot-name)`, whose generation is the slot's write
epoch. Two writers advance it:

- a `set!` (and its undo/redo replay) advances *this* slot, dirtying exactly
  the widgets that read it and nothing else, and
- a pattern sync sweeps the whole `__scene-slot` namespace, handing every
  subscribed slot the newly-current scene's epoch, so the live pattern
  changing dirties exactly the widgets that read any slot.

The sweep is deliberately not an edge on `current-pattern`: that field is the
scene *index*, so it cannot see a pattern whose contents are replaced in place
at the same index (loading a project), which left readers stale. Epochs come
from a process-global counter, so they distinguish two scenes' writes to the
same slot and are re-seeded on load — the sweep therefore repaints on exactly
the resolutions that changed.

No `ui_epoch` bumps; this follows the targeted-invalidation playbook
(undo-drag/glyph-tick lessons). Getting this wrong yields either stale panels
or whole-UI repaints per edit — it is the seam to test hardest.

### Seam 3: boundary capture by name

The shipped-body capture rules (process-channels spec, "Capture rules") gain a
fourth portable reference kind: alongside literals, channel references, and
process handles, **scene-slot references ship by name**. A shipped body
referencing `jb-figures` lowers to a by-name slot read against the scheduler's
resolved current-pattern snapshot:

```lisp
(run jb-figures)     ; in a tick: late-bound per-pattern, like a chan read
```

The same source text means the right thing in both VMs. Scheduler-side reads
are snapshot reads at tick boundaries — processes and ticks never see a
half-written value.

## Storage, resolution, serialization

The store is a per-pattern `name → literal` map, structurally parallel to
`PatternSnapshot.graph_overrides` (`sequencer/state/pattern_snapshot.rs`) and
riding the same plumbing: pattern snapshots, scene track reference state,
scheduler snapshot, track-delete remaps (slots are track-agnostic; remap is a
no-op), and project serialization. Adding the field bumps the project version;
older files load with empty slot stores (all defaults).

Consequences that fall out for free and are the desired semantics:

- Scene duplication copies slot values with the pattern.
- Song/arrangement clips reference patterns, so slots vary per clip exactly as
  graph overrides do.
- Fallback for a pattern with no value is the declaration default — explicit,
  never "last scene's value."

### Publish discipline

Slot writes follow the epoch-before-snapshot ordering rule (scheduler epoch
publish order): bump the relevant epoch **before** publishing the snapshot
containing the new value, or the scheduler reads a one-tick-stale body on
scene switch.

### Consumer-side caching

A slot-driven DSL body may be expensive to normalize (jaki's exact-rational
tables). Consumers must key caches on an explicit **slot epoch** that bumps on
every write — never memoize on first read (the early-chan-resolution staleness
lesson). The store exposes the epoch alongside the value in both VMs.

## Undo

`set!` on a scene slot is a performance gesture and must be undoable. Writes
route through the history host-command pattern (undo-drag playbook): one
command per committed edit carrying (pattern id, slot name, old, new), with
drag-style continuous edits coalescing to a single command on release.
Undo/redo re-dirties via Seam 2's targeted invalidation.

## Naming note: scene vs pattern

The store is per-**pattern**; scenes (and arrangement clips) swap patterns.
`defscene` names the musician's experience, which is the right call for a
surface word — but documentation must say "pattern-scoped, experienced as
per-scene," and the arrangement docs should mention that clips referencing the
same pattern share slot values (exactly like graph overrides; this is a
feature, not a bug).

## Worked example: the jaki builder

```lisp
(defscene jb-figures '((dict :shape (. . . .) :mods ())))

;; Published once at load; the body is data resolved per pattern at tick time.
(alez.jaki.surface/channel-register "builder" :16 (jb-build-body jb-figures))

;; UI panel (gvr-style, but no reactive-set/graph-* two-write dance):
(def jb-add-figure ()
  (set! jb-figures (append jb-figures (list (jb-default-figure)))))

(def jb-set-mod (i mod)
  (set! jb-figures (jb-update-nth jb-figures i
    (lambda (f) (dict-set f :mods (list mod))))))

(effect-buffer "*jaki-builder*" (jb-panel jb-figures SEQ.current-pattern))
```

Scene switch: the tick resolves the new pattern's `jb-figures`, the panel's
reads re-resolve and repaint. Editing while playing: `set!` publishes, the
scheduler picks it up at the next tick boundary.

## Deliberately excluded

- Non-literal values (closures, handles) — the sanctioned pattern for behavior
  stays processes/channels; slots hold data.
- Per-track or per-step scoping (`:track`, plocks) — one new lifetime at a
  time; per-step data is what p-locks are.
- Cross-pattern inheritance chains ("scene 2 falls back to scene 1") —
  resolution is two-level only: pattern override, else declaration default.
- A separate getter/setter API (`scene-slot`, `slot-set!`) — the symbol is the
  API; a parallel functional surface would fork idioms.

## Open questions

- Whether reads outside widget rendering (plain script code in the UI VM)
  should also be reactive-context-aware or always resolve immediately.
  Leaning: immediate resolve; only render-context reads register deps.
- Whether `(ps)`-style introspection should list declared slots and which
  patterns override them (useful for debugging "why does scene 3 sound
  different"). Leaning yes, as a `(scenes)` native.
- Slot value size limits. Serialized per pattern; a pathological slot (a huge
  list) bloats every scene that overrides it. Probably a soft cap with a
  diagnostic rather than a hard limit.
- Interaction with take capture/splice: takes snapshot pattern state — slots
  presumably ride along, but the take lifecycle (release-time stamping) needs
  an explicit pass.
