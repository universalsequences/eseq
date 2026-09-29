# Jaki kind: a GUI for jaki, and the second instance kind

Status: spec rev 1, 2026-09-28. Epic: `bd list --label jaki-kind`.

## 1. Goal

Jaki today is only reachable by live coding a `(jak …)` form. This adds a
point-and-click face, built as an **instance kind** (docs/instance-kinds-spec.md)
so any number of jaki sequencers can live on the project or ride on racks and
kits, exactly like `neural`.

It is also the test of the kind system's generality. `neural` is a graph
sequencer whose document is graph overrides. Jaki is a **generator** (a `:tick`
body) with no graph at all, so the kind system needs two things it does not
have yet:

1. a generator resource slot (`:generator`), next to `:sequencer`;
2. a document home for kinds that are not graphs (`:document`).

Both are kind-agnostic: nothing below names jaki except §5.

## 2. The mapping

```lisp
(jak "hello" :16
  . . -                 ; figures
  -> 0 left             ; row 0: route track 1, modifiers [left]
  -> 1 right accent     ; row 1
  -> 2 (trunc 3) right) ; row 2: [trunc 3] [right]
```

- **Figures** are the pattern: a strip of boxes, one per figure, `[. . -]`
  `[. -]`, with a `+` menu (filterable) listing every figure of length 1..5
  (`.` = 1 unit, `-` = 2). Several figures play in sequence, i.e.
  `(fig (. . -)) (fig (. -))`. Per-figure modifiers (`(every 4 rev)` inside a
  figure) are deliberately out of v1.
- **Rows** are routes, the neuron rows of variable-reset: a route dropdown
  (tracks, or rack pads when rack-owned, plus Off) followed by modifier boxes
  and a `+` menu (filterable) of every route word. A parameterized word shows a
  number picker in its box (`[trunc 3]`), a bare word is a chip (`[left]`).
  Boxes apply in order, left to right, exactly like authored route words.
- A **rows** number picker (1..16, default 8) sets how many rows show; hidden
  rows keep their data, like the neural node count.
- The panel also shows the equivalent `(jak …)` source, read only, so the GUI
  teaches the language.

## 3. `def-kind :document`

```lisp
(def-kind jaki
  :generator (:resolution :16 :requires (alez.jaki.kind) :tick (alez.jaki.kind/tick self))
  :document ((figures '((:dot :dot :dash))) (rows …) (row-count 8))
  :state ((picker-row -1))
  :view jk-panel)
```

`:document` fields are the instance's **document**: per pattern (scene-locked,
like graph overrides and `defscene`), saved with the project, undoable, and
readable by the scheduler. `:state` stays view state (per instance, not saved).

- Values are portable literals (the `defscene` / `ProcessLiteral` vocabulary).
  Defaults are evaluated once by `def-kind`.
- `self.figures` reads, `(set! self.figures v)` writes, same syntax as
  `:state`. Only the storage differs, as `defscene` promised: "the symbol is
  the API".
- **Storage is the scene-slot store** under the reserved slot name
  `%instance/<id>/<field>`. That buys everything `defscene` already has with no
  new store: per-pattern values with a default fallback, scene duplication,
  project serialization, targeted reactive repaint (slot writes and pattern
  switches), `scene-slot-history-write` undo, and the scheduler's per-chunk
  slot snapshot.
- **Scene-locked like graph overrides.** A project-owned instance's document
  lives in each scene's slot store. A rack-owned instance on a rack with a
  clip bank keeps it in the rack clip (`RackClip::scene_slots`), exactly
  where its graph overrides would live: scenes sharing a clip share it,
  relaunching a clip brings its document, a new scene forks it. One read
  (`ProjectScenes::composed_scene_slots`: the scene's store with that rack's
  instance slots taken from the pointed clip) serves the UI, the scheduler
  snapshot and its per-scene table; one write route
  (`scene_slot_store_mut`) serves `set!`, undo and redo. The scene bank knows
  owners through `ProjectScenes::instance_racks`, which the App syncs from its
  instance list.
- In eseqlisp a document field is a new `FieldSlot::Document`. Reads and writes
  call the host natives `__instance-doc-read` (id, field, default) and
  `__instance-doc-write` (id, field, value) when registered, so the scene-slot
  reactive edge and history command come from the host exactly as for a
  `defscene` read; without a host (unit tests) they fall back to a local cell.

### 3.1 Lifecycle

The instance lifecycle edits learn about document slots:

| Edit | Document |
|---|---|
| create | nothing written: every pattern reads the defaults |
| duplicate | every pattern's `%instance/<src>/…` slots copied to the new id |
| delete | every pattern's slots removed; undo restores them |
| kit export / load | the current pattern's resolved document travels with the instance and lands in every pattern under the fresh id |
| id allocation | also skips ids any pattern still holds document slots for |
| move owner | each scene's composed document is re-homed like graph overrides (scene stores ↔ the rack's clips); values are verbatim, so a route keeps its index |
| kit export / load (clips) | each clip's instance slots travel in `ProjectRackClip::scene_slots`, re-keyed to the fresh id |

## 4. `def-kind :generator`

The second resource slot. The body is the `def-sequencer` generator body
(`:resolution`, `:requires`, `:tick`) captured as data. Each instance publishes
a tick-mode `PublishedSequencer` whose id is the instance id and whose tick
source is

```lisp
(let ((self (__instance-document <id> "<kind id>"))) <tick body>)
```

so `self.figures` in the tick reads the field. On the scheduler VM
`__instance-document` returns a map of the instance's document fields resolved
from the chunk's scene-slot snapshot (defaults from the host kind registry),
and the existing `GetField` on a map does the rest. A kind may have
`:sequencer` or `:generator`, not both.

**Rack ownership.** `PublishedSequencer` gains `owner_rack`. When it is set,
the scheduler maps a generator emission's `:track` through the rack's members
(track `n` = pad `n`), the same member-relative rule graph routes use, so a kit
with a jaki instance plays its own pads wherever it lands. A member index past
the rack's size emits nothing.

## 5. The jaki kind (`alez.jaki.kind`)

Document (all literals):

```lisp
figures   ; list of figures, each a list of :dot / :dash, e.g. ((:dot :dot :dash))
rows      ; list of (dict :route n-or--1 :mods ((dict :op "trunc" :args (3)) …))
row-count ; how many rows show and play
```

Row `i` beyond `(len rows)` is the default row (route off, no mods). A fresh
instance has one figure `(. . . .)` and row 0 on track/pad 1.

The tick builds ordinary jaki body data from the document (`:dot` → `'.`,
op strings → route-word symbols via a `match` table) and calls
`alez.jaki.core/run`, so every evaluator rule, hand model and memo is the
language's own. Rows routed Off and rows past `row-count` are left out; no
live row means silence.

Modifier catalog: every route word — `left right accent rev stac ghost swap`,
the one-number words `trunc rot shift fast slow gate vel note`, the velocity
model `basevel dotdecay dashdecay minvel maxvel`, `every n <word>`,
hand-scoped `L <word>` / `R <word>`, and `split` / `merge` with a
`first | last | all` target. Figure transforms used as route words apply to
each figure of the route in place (`rev` on `(. . -) (. -)` gives
`(- . .) (- .)`), which is why route words now accept every figure transform
(`alez.jaki.core/route-step-word` falls back to `xform`). `(cyc …)` args,
`mute`/`solo` control routes and per-figure modifiers are follow-ups.

Resolution is fixed at `:16` by the kind in v1 (the published resolution is
static); a per-instance resolution needs republish-on-change and is a
follow-up.
