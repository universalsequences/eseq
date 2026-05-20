# Patcher Widget — Spec

A visual patch editor for authoring `dsp.lisp` files (instruments and effects)
inside eseq, with bidirectional lisp ↔ patch projection, hot-reload, and
defmacro-as-subpatch.

---

## 1. Motivation

Typing dgenlisp by hand inside the app is friction-heavy. A visual patcher
makes the same authoring loop fast for the common case (wiring oscillators,
filters, envelopes, allpasses, feedback tanks), while keeping the lisp file as
the source of truth so that:

- Existing instruments/effects already shipped as `dsp.lisp` open in the
  editor with no migration.
- The agent and the user share one representation. Agent-written lisp opens
  visually; user-edited patches read as lisp the agent can refactor.
- Power-users can drop to text whenever the visual representation is
  inconvenient (advanced macros, tensor work) without leaving the workflow.

The patcher is not a new compiler. It is a **source generator with hot-reload**
sitting on top of the existing `lisp_effect` / `compile_instrument` pipeline.

---

## Current Progress

The patcher currently exists as an `eseqlisp` widget that can be embedded with:

```lisp
(patcher
  :intent :instrument ; or :effect
  :path "instruments/.../dsp.lisp")
```

Implemented so far:

- Read-only import/projection from `dsp.lisp` into a patch graph.
- Top-level `def`, nested calls, `in`, `out`, `param`, destructuring defs,
  `defmacro`, and history projection.
- Unsupported forms render as explicit code-island nodes instead of being
  silently dropped.
- Operator metadata comes from
  `crates/sequencer/tools/dgenlisp-operators.json`.
- `param` forms render as normal parameter nodes, and symbol references to
  params become patch cables into consuming operators.
- `make-history`, `read-history`, and `write-history` collapse into one
  `history` node with feedback-class cabling.
- Macro instances render as normal nodes; pressing Enter on a selected macro
  opens its read-only subpatch. The breadcrumb/back-arrow returns to the root.
- Macro subpatch arguments render as `in 1`, `in 2`, etc., and macro return
  values render as `out 1`, etc.
- Inline node argument display supports hidden literal inlets and `?`
  placeholders for connected args before later literals, e.g. `ap ? 0.6`.
- Deterministic auto-layout plus in-memory node position overrides.
- Smooth canvas-local Metal cables, rounded node chassis, top/bottom
  semicircle ports, grid, breadcrumb, diagnostics, panning, node selection, and
  node dragging.
- Double-clicking the background creates an in-memory draft node.
- Double-clicking a node edits its header text in memory using the shared text
  input cursor/selection math.
- Pressing Enter commits an edited draft/header into the in-memory graph shape
  and resolves visible ports from operator metadata.
- Focused patcher key input invalidates layout/redraw correctly, so text
  editing cursor state updates immediately.
- Headless capture support exists through `eseqlisp_capture`, including an
  ignored visual fixture test for the lexilush patcher screenshot.

Important current limitations:

- Edits are in-memory only. The patcher does not emit Lisp or save `dsp.lisp`.
- No `dsp.layout.json` sidecar is written yet; node positions are not persisted.
- No hot-reload or compile loop is connected to patcher edits.
- New/edit node behavior only updates standalone node text/ports; it does not
  yet update source-level bindings, output routing, or surrounding expressions.
- Cable creation, cable deletion, reconnecting ports, and node deletion are not
  implemented yet.
- Macro editing is navigable/read-only at the source level; edited macro view
  contents do not round-trip to `defmacro` source yet.
- Comments are still import diagnostics/non-editable because the current parser
  does not preserve comments.
- No zoom, minimap, search, operator autocomplete, or palette UI yet.
- No persistent undo/redo transaction model yet.

Recommended next phase:

1. Introduce a real patch edit model that distinguishes imported source nodes,
   draft nodes, committed unsaved nodes, and deleted nodes.
2. Implement graph mutation operations for create/edit/delete node and
   create/delete/reconnect cable.
3. Add deterministic graph-to-DGenLisp emission for the representable subset,
   initially behind an explicit save/apply action.
4. Add `dsp.layout.json` persistence for user-positioned nodes and pan state.
5. Wire save/apply into the existing compile/load path once emission is stable.

---

## 2. Goals / Non-Goals

### Goals

- One widget, `patcher`, embeddable in eseqlisp UI scripts.
- Modes: `instrument` and `effect`. Modes differ only in their immutable
  plumbing (entry/exit nodes).
- Hot-reload on every meaningful edit, preserving parameter values where the
  parameter identity is unchanged.
- defmacro round-trip: a `defmacro` in lisp opens as a navigable subpatch; a
  subpatch saves back as a `defmacro`.
- Sugiyama auto-layout for any lisp the editor has never seen.
- Sidecar layout file (`dsp.layout.json`) appears only after the first
  user edit in the patch view.
- Inline node syntax: arguments typed in the node header bake into the lisp;
  `?` keeps an inlet visible; missing trailing args keep those inlets visible.
- `@key value` annotations supported on nodes for the existing dgenlisp `@`
  attributes (e.g. `@name`, `@modulator`, `@min`, `@max`, `@default`,
  `@mod-mode`).
- Unknown operator names render the node in an error state (red border) and
  block compilation until resolved.

### Non-Goals

- **No UI/ui.lisp generation.** This widget edits only `dsp.lisp`. The
  instrument's control surface (`ui.lisp`) is authored separately (by hand or
  by the agent).
- No new compiler, no new IR, no new feedback analysis. All semantic checking
  is delegated to the existing `compile_lisp` / `compile_instrument` paths.
- No general-purpose graph editor. The patcher's only output is dgenlisp text.
- No multi-document or comparison view in v1. One patch at a time.
- No persistent "patch format" distinct from lisp. The lisp file *is* the
  patch.

---

## 3. Architecture Overview

```
┌──────────────────────────┐         ┌────────────────────────┐
│   patcher widget         │  emit   │   dsp.lisp (on disk)   │
│  (node graph in memory)  ├────────▶│                        │
│                          │ parse   │                        │
│                          │◀────────┤                        │
└────────┬─────────────────┘         └──────────┬─────────────┘
         │ on edit (debounced)                   │
         ▼                                       ▼
┌──────────────────────────┐         ┌────────────────────────┐
│  emit dsp.lisp text      │         │   compile_instrument / │
│  → compile_and_load_*    │ ──────▶ │   compile_and_load     │
│  → hot-swap dylib        │         │   (existing pipeline)  │
└──────────────────────────┘         └────────────────────────┘
```

**Single source of truth:** the lisp text on disk. The patch graph in memory
is a *projection* of that text. The sidecar `dsp.layout.json` carries only
positional metadata (x, y per node, view state per subpatch); it is never
required for compilation or correctness.

The widget owns:
- Parse: lisp → graph (using existing dgenlisp parser).
- Emit: graph → lisp (deterministic, stable ordering).
- Layout: auto-layout for fresh imports, persisted positions for user-edited
  graphs.
- Rendering: minimal Metal scene of nodes, ports, cables, header text.
- Input: keyboard (text entry, autocomplete), mouse (drag wires, drag nodes,
  double-click navigate).

---

## 4. Data Model

```
Patch
├── mode:            Instrument | Effect
├── plumbing:        Vec<PlumbingNode>      // immutable, mode-determined
├── defmacros:       Vec<MacroDefinition>
├── nodes:           Vec<Node>              // root-level
├── connections:     Vec<Connection>
├── histories:       Vec<HistorySlot>       // make-history declarations
├── params:          Vec<ParamDecl>         // (param ...) forms
└── view_stack:      Vec<ViewFrame>         // breadcrumb: root → macro → …

Node
├── id:              StableId               // see §11
├── op:              String                 // operator name OR macro name
├── kind:            Builtin | MacroInstance | Param | Out | In
├── args:            Vec<Arg>               // Literal(value) | Open | Question
├── attributes:      Vec<(String, Value)>   // @key value pairs
├── position:        (f32, f32)             // from layout sidecar or auto
└── error:           Option<String>         // unknown op, arity, etc.

Connection
├── from:            (NodeId, OutletIdx)
├── to:              (NodeId, InletIdx)
└── kind:            Audio | Control | Feedback

MacroDefinition
├── name:            String
├── params:          Vec<String>            // parameter names
├── body:            Patch                  // recursive — a subpatch
└── inline_render:   bool                   // §12.5
```

A subpatch is just a `Patch` nested under a `MacroDefinition`. The same data
structure renders at the root and inside a defmacro view; only the immutable
plumbing differs (root has instrument/effect I/O; subpatch has parameter
inlets and one or more return outlets — see §10.3).

```
Comment
├── id:              CommentId
├── anchor_node:     NodeId           // node this comment is tied to
├── text:            String
├── offset:          (f32, f32)       // bubble position relative to anchor
└── kind:            Line | Section    // affects emit position (§7.x)
```

---

## 5. File Model

Each instrument/effect lives in its existing folder:

```
crates/sequencer/instruments/videogame-arp/
├── dsp.lisp           ← source of truth (already exists)
├── ui.lisp            ← out of scope for patcher
└── dsp.layout.json    ← NEW; written on first edit in patcher
```

`dsp.layout.json`:

```json
{
  "version": 1,
  "root": {
    "nodes": {
      "amp_env":    { "x": 320.0, "y": 180.0 },
      "pitch_env":  { "x": 320.0, "y": 240.0 },
      "ap#0":       { "x": 480.0, "y": 120.0 }
    },
    "scroll": { "x": 0.0, "y": 0.0 },
    "zoom":   1.0
  },
  "macros": {
    "ap": {
      "nodes": { "node": { "x": 200, "y": 100 }, "delayed": { "x": 400, "y": 100 } },
      "inline_render": false,
      "scroll": { "x": 0.0, "y": 0.0 },
      "zoom":   1.0
    }
  }
}
```

**No semantic content in the layout file.** If it is deleted or corrupted, the
patch still loads (with auto-layout). If a hand-edit to `dsp.lisp` removes a
node, its entry in the layout file is silently dropped on the next save.

---

## 6. Lisp ↔ Patch Mapping

| Lisp form                                  | Patch representation                                  |
| ------------------------------------------ | ----------------------------------------------------- |
| `(def foo (op a b c))`                     | Node `foo`, op = `op`, args = `[a, b, c]`             |
| `(op a b c)` (anonymous, inside larger)    | Anonymous node, args = `[a, b, c]`                    |
| `(in N @name x)`                           | Plumbing **entry** node (immutable in instrument)     |
| `(out expr N @name y)`                     | Plumbing **exit** node (immutable in instrument)      |
| `(param p @default … @min … @max …)`       | Param node — appears as a labeled control input       |
| `(defmacro name (a b c) …body…)`           | Subpatch definition `name` (see §10.3 for multi-out)  |
| `(name arg1 arg2)` (macro invocation)      | Subpatch **instance** node                            |
| `(def (a b) (op …))`                       | Destructure: `op` has 2 outlets `a`, `b`              |
| `(make-history h)`                         | Implicit — declares a `HistorySlot`                   |
| `(write-history h x)`                      | Endpoint of a feedback cable; sink half of the slot   |
| `(read-history h)`                         | Endpoint of a feedback cable; source half of the slot |
| `; comment` (preceding a `(def …)`)        | Comment bubble tied to that node (§7.3)               |

Feedback cables (history pairs) are rendered as **dashed** wires and routed
distinctly from forward cables (Sugiyama treats them as back-edges).

### 6.1 No inline subexpressions

Every nested call becomes its own node. There are no inline arg expressions.
A line like:

```lisp
(def pitch_semi (+ (* arp_semi arp_amount)
                   (* pitch_env pitch_env_amt)
                   (* lfo vibrato_depth)))
```

renders as **four nodes** (three `*`, one `+`), not one. This is the same
constraint Max/MSP's `gen~` enforces. The cost is denser-looking patches;
the benefit is a single textual-vs-wire representation per operation, no
ambiguity, no header-side expression parser, no round-trip surprises.

Tensors and other multi-output ops follow the same rule: they are just
nodes. Outlet count and labels come from the op table (§14.1).

Args in a node header are restricted to:

```
literal := NUMBER | STRING | SYMBOL | ARRAY
ARRAY   := "[" (NUMBER (WS NUMBER)*)? "]"
```

(Array literals are needed because dgenlisp uses them for tensor shape and
data: `@shape [1024]`, `@data [0.02 0.04 …]`. See
`spectral-bin-freeze/dsp.lisp`.)

### 6.2 Emit ordering

When emitting lisp from the graph, the emitter performs a topological sort and
preserves the user's `def` names verbatim. Hand-edited lisp opens; round-trip
through the editor without graph changes produces a byte-identical file (modulo
whitespace normalization).

---

## 7. Immutable Plumbing

The patcher enforces a fixed set of entry/exit nodes per mode. These nodes
cannot be deleted, renamed, or have their port count changed.

### Instrument plumbing

```
(in 1 @name gate)          ─▶ [gate]
(in 2 @name pitch)         ─▶ [pitch]
(in 3 @name velocity)      ─▶ [velocity]
(in 4 @name trigger)       ─▶ [trigger]
(in 5..10 @name modN @modulator N)   ─▶ [mod1]…[mod6]

(out … 1 @name audio)      ◀─ [audio out]
```

### Effect plumbing

```
(in 1 @name signal-l)      ─▶ [in-l]
(in 2 @name signal-r)      ─▶ [in-r]

(out … 1 @name out-l)      ◀─ [out-l]
(out … 2 @name out-r)      ◀─ [out-r]
```

The plumbing nodes render with a distinct visual treatment (locked icon, muted
background). They can be repositioned but not removed.

A patch is **emittable** even with unconnected exit nodes; the emitter inserts
`(out 0 N @name …)` as a placeholder so the file still compiles. This keeps
hot-reload functioning while the patch is mid-construction.

### 7.3 Comments

All comments are **node-anchored bubbles**. There is no free-floating
comment, no zone/region abstraction.

- Right-click a node → "Add comment" creates a bubble next to the node with
  a dashed leader line pointing at the node. The user types into the bubble.
- The bubble can be dragged; its position is stored as an offset from the
  anchor node in `dsp.layout.json`. Moving the anchor moves the bubble.
- Detaching: drag the leader-line endpoint onto a different node to re-anchor.

Emit rule:

- A comment's text is emitted as one or more `; …` lines **immediately
  above** the anchor node's `(def …)` form in the lisp output.
- Multi-line bubbles emit one `;` line per line of text.
- If multiple bubbles anchor the same node, they emit in stable order
  (top-to-bottom by bubble Y offset).

Section headers like `; Initial Diffusion Stage` from `lexilush/dsp.lisp`
are not a separate concept — the user anchors the bubble to the first node
of the section. When that node moves, the section label moves with it. This
is an acceptable lie because in practice users rearrange whole sections
together.

Top-of-file prose (e.g. the three-line summary at the top of
`videogame-arp/dsp.lisp`) is anchored to a synthetic "file" node that is
not rendered in the canvas but exists for emit purposes. Editing it happens
through a "File description" menu item.

---

## 8. Node Creation & Editing

### 8.1 Empty node

- Double-click empty canvas → empty node appears under cursor with focus in
  its header text field. Cursor is in insert mode at column 0.
- `Esc` cancels and removes the empty node.
- Clicking outside while the node is empty removes the node.

### 8.2 Inline header syntax

The node header is a single text line. Its grammar:

```
header  := op-name (WS arg)* (WS attribute)*
arg     := literal | "?"
literal := NUMBER | STRING | SYMBOL
attribute := "@" KEY (WS value)?
```

Examples:

| Typed                                | Resulting node                                        |
| ------------------------------------ | ----------------------------------------------------- |
| `biquad`                             | `biquad`, all inlets visible (arity inferred)         |
| `biquad 4432`                        | `biquad`, arg 0 = `4432` (hidden); inlets 1..N open   |
| `biquad ? 4432 1 1 0`                | `biquad`, inlet 0 forced open; args 1..4 baked        |
| `biquad ? 4432 1`                    | inlet 0 open; args 1,2 baked; inlets 3,4 open         |
| `param cutoff @default 6200 @min 80` | `param` node `cutoff` with attribute list             |
| `out @name audio`                    | only valid on plumbing; otherwise error               |
| `defmacro ap`                        | declares subpatch `ap`; enters it on Enter (§12)      |

Rule of thumb:

- A **literal at position N** → inlet N is hidden and the value is baked into
  the emitted lisp.
- A **`?` at position N** → inlet N stays visible; the user can wire into it.
  Emitter inserts a placeholder (`0` or the operator's documented default).
- A **missing trailing position** → inlet stays visible.

### 8.3 Autocomplete

As the user types in a node header:

- After ≥1 character in the op-name slot, a popup shows matching operators
  ranked by (1) prefix match, (2) substring match, (3) edit distance.
- `Tab` accepts the highlighted suggestion and advances to the first arg.
- `Enter` commits the node, regardless of completion state.
- The autocomplete list includes:
  - All dgenlisp builtins (from the existing operator registry).
  - All in-scope defmacros (declared in this patch).
  - All in-scope param/def names that can be referenced as values.

When the cursor is on an `@` attribute, autocomplete switches to attribute
keys valid for the current operator (e.g. `@name`, `@modulator` for `in`;
`@default`, `@min`, `@max`, `@unit`, `@mod`, `@mod-mode` for `param`).

### 8.4 Error state

On `Enter`:

- If the operator name resolves (builtin or in-scope macro): node is
  committed normally.
- If the operator name does **not** resolve: node is committed in **error
  state** — red border, red header text, tooltip with the failure reason.
  The patch still saves to disk (so the user can refactor in the lisp), but
  compilation is suppressed until all error-state nodes resolve.

Arity mismatches and attribute typos are surfaced the same way, with the
specific message from the dgenlisp compiler.

### 8.5 Editing existing nodes

- Click on a node header to enter edit mode; same grammar as creation.
- `Esc` cancels edits; `Enter` commits.
- Editing an existing `def` name renames the lisp `def` and updates all
  references in scope. (This is a textual rename — done at emit time using
  the graph's reference index, not by post-hoc string substitution on disk.)

### 8.6 Deleting

- `Backspace` / `Delete` on a selected node removes it and all incident
  connections. Plumbing nodes refuse deletion (briefly flash).

---

## 9. Wiring

- Drag from outlet → inlet creates a connection.
- Drag from inlet → outlet creates a connection (symmetric).
- An inlet accepts at most one connection. Dragging a new wire to a connected
  inlet replaces the existing wire.
- An outlet may fan out to many inlets.
- Wires render as bezier curves. Forward wires solid; feedback wires
  (history-bridged) dashed.
- Right-click on a wire → context menu (delete, insert node mid-wire).

### 9.1 Feedback (histories)

The user does **not** manually invoke `make-history`. Instead:

- Drag a wire from an outlet that creates a cycle in the graph → editor
  detects the cycle, automatically synthesizes a `make-history` slot, and
  emits a `write-history`/`read-history` pair on save. The wire renders
  dashed.
- Deleting a dashed wire removes the history slot.

This mirrors the `lexilush` figure-eight tank cleanly: the user draws the
loop visually; the emitter inserts the four `make-history` slots and six
read/write calls.

---

## 10. Defmacro (Subpatch) Workflow

### 10.1 Creating a defmacro

Two paths:

1. **Inline declaration.** Type `defmacro ap` in an empty node, press
   `Enter`. The node converts into a subpatch-definition marker (top-of-file
   declaration), and the view drops into the macro body with the cursor on
   an empty canvas. The macro's plumbing is auto-derived from its parameter
   list (one inlet per param, one outlet for the return value).
2. **Promote selection.** Select N nodes → `Cmd+Shift+G` ("group into
   defmacro") → prompts for a name and parameter list (defaults: inlets
   become params, outlet becomes return). The selection is replaced by a
   single subpatch-instance node.

### 10.2 Navigation

- Double-click a subpatch-instance node → view descends into the macro body.
  A breadcrumb appears at the top of the widget: `[root] / [ap]`.
- Click any breadcrumb segment → return to that level.
- `Esc` at the canvas level (no node focused) ascends one level.
- The viewport scroll/zoom is preserved per view-frame on the stack.

### 10.3 Subpatch parameters and return values

Inside a macro view, the macro's parameter list renders as **immutable
plumbing** on the left edge (one node per param, named accordingly), and
one or more "return" plumbing nodes on the right edge. These behave
identically to instrument/effect plumbing — locked, repositionable,
undeletable.

Editing the macro's parameter list (only possible by editing the macro's
declaration node) updates the inlet plumbing.

**Multi-output macros.** A macro's outputs are determined by the body's
final form:

- A bare symbol (e.g. `delayed`) → one outlet, named `delayed`.
- A bare tuple of symbols (e.g. `(re im)` or `(out1 out2 out3)`) → N
  outlets, named by the symbols in order.

Example:

```lisp
(defmacro split-band (sig cut)
  (def lo (svf sig cut 0.7 0))
  (def hi (svf sig cut 0.7 1))
  (lo hi))                       ; ← bare tuple = two outlets named lo, hi
```

Invocation site uses destructuring `def`:

```lisp
(def (low high) (split-band input 800))
```

Note: this requires dgenlisp to accept a bare tuple `(a b)` as a macro
body's last form and emit it as multi-value return. A small compiler tweak,
to be confirmed before Phase 3.

Adding a return outlet from inside the macro view: drag a wire from any
node's outlet to the right edge of the canvas → editor adds a new return
plumbing node and prompts for its symbol name. Emitter updates the body's
final form accordingly.

### 10.4 Instance vs definition: duplicate semantics

Two distinct operations, both keyboard-driven:

| Operation         | Shortcut         | Result                                            |
| ----------------- | ---------------- | ------------------------------------------------- |
| Duplicate instance| `Cmd+D`          | New `(macro-name …)` call. Same defmacro shared. |
| Fork definition   | Right-click → Fork | Macro body copied to `name-2`; selection retargeted. |

Right-click on a subpatch instance also shows "Fork into private copy" and
"Open definition." Without this distinction, users will accidentally edit-all
when they meant edit-one.

### 10.5 Inline rendering for trivial macros

A macro qualifies for **inline rendering** when:

- Body has zero internal `def` forms, AND
- Body has zero histories, AND
- Body is a single expression.

`semi_ratio` and `quantize` from `videogame-arp/dsp.lisp` qualify.

Inline-rendered macros render as a labeled single-line node (like a builtin),
with their arg list directly in the header. Double-click still descends into
their (single-line) body for editing.

The qualification check runs on every edit; promoting a previously-trivial
macro by adding a `def` automatically switches it to full subpatch rendering.

---

## 11. Node Identity (Stable IDs)

Layout positions need stable keys that survive renames and reorderings.

Rule:

- A `def`'d node's stable ID is its `def` name (e.g. `amp_env`,
  `tank-l-mod`).
- An anonymous node's stable ID is `<op>#<index>`, where `<index>` is the
  zero-based occurrence order of that operator in the lisp file
  (e.g. `ap#0`, `ap#1`, `biquad#0`).
- Plumbing nodes use their `@name` (e.g. `gate`, `audio`, `in-l`).
- Subpatch instances of macro `M` are `M#<index>`.
- History slots use their declared name (`h-loop-l`, etc.).

On open, the editor reconciles `dsp.lisp` with `dsp.layout.json`:

1. Match by stable ID. Position the matched nodes.
2. Unmatched lisp nodes → auto-layout into empty regions.
3. Unmatched layout entries → silently dropped.
4. Write the merged layout back on first interaction.

When a user renames a `def` in the editor, the rename is propagated through
the graph and the layout entry is moved to the new key, so identity survives
renames. Hand-edits to the lisp that rename a `def` will lose that node's
position (treated as unmatched on next open); this is acceptable because the
auto-layout fallback exists.

For robustness when reordering anonymous nodes is common, v2 may emit
sentinel comments (`; @id ap-7`) before each form to anchor identity. Not in
v1.

---

## 12. Hot-Reload

### 12.1 Trigger

Every committed edit (commit = `Enter`, deselect, blur, click outside)
schedules a debounced reload after **80 ms** of idle. Drag-while-wiring does
not trigger reload until release.

### 12.2 Pipeline

```
edit → emit dsp.lisp text → write to disk →
  if Instrument: compile_and_load_instrument(text, sr)
  if Effect:     compile_and_load(text, sr)
→ on success: swap dylib in audio thread (existing path)
→ on failure: surface error, mark offending node(s) red,
              keep previous dylib live
```

The patcher does **not** invent its own compile loop. It calls the existing
entry points in `crates/sequencer/src/lisp_effect.rs`
(`compile_and_load`, `compile_and_load_instrument`).

### 12.3 Parameter stability

When the new compile succeeds and the instrument/effect is already live, the
hot-swap path must preserve current param values for params whose `(param
foo …)` declaration is unchanged across the swap. Identity is by `name` (the
symbol after `param`).

- New params (declared in this compile but not the previous) take their
  `@default`.
- Removed params (in previous but not current) are dropped.
- Existing params keep their current value, even if `@min`/`@max`/`@unit`
  changed (clamped to the new range on the next host write).

This is the live-coding feel the user wants: tweak a node, the synth keeps
playing, the knob the user is holding stays where it is.

### 12.3.1 Param defaults: `@mod` / `@mod-mode`

The dgenlisp compiler is updated so that a `(param …)` form with no `@mod`
attribute defaults to `@mod true @mod-mode additive`. The patcher does not
emit those attributes when they match the new defaults — the emitted lisp
stays clean. A user who wants a non-modulatable param writes `@mod false`
explicitly, and the editor surfaces that as a toggle in the param node body.

This is a compiler-side change shipped alongside the patcher. Existing
files that relied on missing `@mod` meaning "not modulatable" will flip
behavior on re-compile; a one-time audit of in-repo instruments/effects
catches any regressions.

### 12.4 Bad states

If the emitted lisp fails to compile:

- The previous dylib stays live.
- The offending nodes (per the compiler's error spans) are marked with a red
  outline and tooltip.
- The patch on disk is **still written** — the lisp file may be broken,
  matching what would happen if the user typed the same thing in the text
  editor. The patcher does not gate persistence on compilation success,
  because that would prevent the agent from collaborating mid-edit.

---

## 13. Sugiyama Auto-Layout

Used when:

- Opening a patch with no `dsp.layout.json`.
- Reconciling unmatched nodes (§11.3).
- User invokes "Re-layout" (menu / shortcut).

Algorithm: standard Sugiyama (4-phase: cycle removal, layer assignment, vertex
ordering, x-coordinate assignment). Feedback edges (history pairs) are
identified up-front and routed as back-edges.

Defaults:

- Layer spacing: 160 px horizontal.
- Node spacing within layer: 24 px vertical.
- Plumbing entry nodes pinned to layer 0 (leftmost).
- Plumbing exit nodes pinned to the last layer (rightmost).

Recompute is incremental on hand-edited lisp imports; not on every user
interaction (we trust the user's positions thereafter).

---

## 14. Widget Implementation

### 14.1 Location

New file: `crates/eseqlisp/src/widget_render/patcher.rs`.

Registered alongside the existing widgets (`timeline`, `transport_clock`, …)
via `register_widget_natives` in `crates/eseqlisp/src/widgets.rs`.

### 14.2 Lisp surface

```lisp
(patcher
  @intent     instrument           ; or 'effect
  @path       "instruments/videogame-arp/dsp.lisp"
  @on-compile (lambda (ok msg) ...)
  @on-error   (lambda (errs)     ...))
```

Returns a widget instance. Resizes to fill its parent (like `timeline`).

### 14.3 State

Implements `WidgetDefinition` (per `widget_render/mod.rs`). Owns:

- `Patch` (root graph).
- `view_stack: Vec<ViewFrame>` — current navigation depth.
- `viewport: (scroll_x, scroll_y, zoom)` per view-frame.
- `selection: Vec<NodeId>`.
- `interaction: Idle | EditingHeader | DraggingNode | DraggingWire | …`.
- `pending_reload: Option<Instant>` — debounce timer.
- `compile_error: Option<…>` — last failed compile.

### 14.4 Rendering

Closest analog: `timeline.rs` (smooth scrolling, custom Metal primitives,
keyboard+mouse, ~3.6k lines).

Render passes (minimal shader count):

1. **Background grid** (one `MetalQuadPrimitive` per visible grid line, or a
   single shader-drawn grid quad).
2. **Cables** (bezier curve, dashed for feedback). One `MetalPrimitive`
   variant `Cable` with start/end/control points + style.
3. **Nodes** (rounded rect with header). Composed of:
   - One `MetalRectPrimitive` for body.
   - One `MetalProportionalTextPrimitive` for the header.
   - One `MetalQuadPrimitive` per visible port (small circle / triangle).
4. **Selection overlay**, **error overlay** (red outline rect).
5. **Breadcrumb bar** at top — text primitives.
6. **Autocomplete popup** — same primitives as a dropdown widget; reuse
   `widget_render/dropdown.rs` patterns where possible.

All rendering happens inside the widget; no node is itself a sub-widget. This
keeps the widget self-contained and avoids the layout-tree overhead the user
called out.

### 14.5 Input

Mouse:

- LMB-down on empty canvas → start marquee select (drag) OR (if `dblclick`)
  spawn empty node.
- LMB-down on node → start node drag (with selection).
- LMB-down on outlet → start wire drag.
- LMB-down on inlet → start wire drag (reverse).
- LMB-down on header text → enter header-edit mode.
- RMB → context menu.
- Wheel → vertical scroll; Cmd+Wheel → zoom; Shift+Wheel → horizontal.

Keyboard (when widget has focus, no header in edit mode):

| Key                | Action                                |
| ------------------ | ------------------------------------- |
| `Enter` (on empty) | spawn node at cursor / center         |
| `Cmd+D`            | duplicate instance                    |
| `Cmd+Shift+G`      | group into defmacro                   |
| `Cmd+Z` / `Cmd+Shift+Z` | undo / redo (graph-level history) |
| `Esc`              | ascend one view-frame OR clear selection |
| `Backspace`        | delete selection                      |
| `Cmd+L`            | re-layout (Sugiyama)                  |
| `Cmd+S`            | force flush (skip debounce)           |
| `Cmd+/`            | toggle sticky note on selection       |

When a header is in edit mode, the widget consumes most keys for text entry;
only `Esc`, `Enter`, `Tab`, arrow keys behave specially.

---

## 15. Phasing

### Phase 0 — research & schema

- Inventory dgenlisp operators (port counts, names, optional args). Output a
  static table `crates/eseqlisp/src/widget_render/patcher/ops.rs` (or
  generate from existing operator registry if one exists).
- Spec the node identity rules and ensure the parser/emitter agree.

### Phase 1 — read-only viewer

- Load `dsp.lisp` → parse to graph → Sugiyama layout → render.
- No editing. Plumbing rendered immutably. Subpatch navigation works.
- Validate against `videogame-arp/dsp.lisp` and `lexilush/dsp.lisp` (the
  two examples we walked through).

### Phase 2 — editor

- Empty-node creation, header grammar, autocomplete.
- Wire drag (forward only).
- Hot-reload pipeline wired to existing `compile_and_load_*` entry points.

### Phase 3 — feedback & macros

- Cycle detection → automatic `make-history` synthesis.
- `defmacro` workflow: declaration, navigation, instance vs fork, inline
  rendering for trivial macros.

### Phase 4 — polish

- Sidecar `dsp.layout.json` reconcile-on-open.
- Param stability across hot-swap.
- Undo/redo at the graph level.
- Sticky notes from `;` comments.

### Phase 5 — agent integration

- Agent tool surface: "open patch", "describe patch", "edit patch" —
  expressed against the lisp, not the graph (the graph is a projection, no
  separate API needed).

---

## 16. Decisions Locked

All five originally-open questions are now resolved.

1. **No inline arg expressions.** Every nested call is its own node. Same
   constraint as Max/MSP `gen~`. Header grammar restricted to literals
   (number, string, symbol, array) and `?` placeholders. See §6.1.
2. **Comments are node-anchored bubbles.** Right-click a node → add bubble
   with a dashed leader line. Emits as `; …` line(s) above the anchor
   node's `(def …)`. Section headers attach to the first node of the
   section. No free-floating comments, no zone abstraction. See §7.3.
3. **Tensors are not special.** Just nodes, with outlet counts from the op
   table. Compile errors surface as red node outlines; the error parser
   maps compiler spans back to nodes the same way it does for any other op.
4. **Param defaults `@mod true @mod-mode additive`.** dgenlisp compiler is
   updated to default `@mod` to true (additive). Editor never emits those
   attrs when they match defaults. `@mod false` is the opt-out. See
   §12.3.1. One-time audit of existing files required before shipping.
5. **Multi-output via final-form tuple.** A defmacro body whose last form
   is a bare tuple `(a b c)` exposes N outlets named by those symbols.
   Builtins with multiple outlets (e.g. `fft`) declare their outlet names
   in the op table. Destructure at the call site uses `(def (a b) (op …))`.
   Small dgenlisp compiler tweak required to accept bare-tuple body
   returns. See §10.3.

Remaining ambiguities deferred to v2:

- **Sentinel-comment node IDs.** If anonymous-node index drift becomes a
  real problem, emit `; @id <op>-<n>` markers as stable anchors. Not
  needed until users complain.
- **Comment re-anchoring UX.** Dragging a leader line to a new anchor is
  the spec; the precise affordance (handle on the line endpoint vs.
  modifier-click vs. menu) decided in implementation.
- **Code-island fallback for compile errors.** If a specific operator turns
  out to be impractical to render as a node (no current candidates), the
  v2 escape hatch is a read-only text-region node spanning that subtree.
  Not built unless needed.

---

## 17. Why this is tractable

A standalone visual patcher is a large project (the Swift editor is ~30k
LOC). This patcher is small because:

- **No compiler.** The existing dgenlisp toolchain does everything semantic.
- **No new IR.** The lisp file is the IR.
- **No new state model for live coding.** `compile_and_load_*` already
  exists and is battle-tested.
- **No new operator catalog.** Operators are whatever dgenlisp accepts.
- **No new feedback analysis.** Cycle detection is a 50-line graph algorithm
  whose output is just two helper forms (`write-history` / `read-history`)
  the compiler already understands.
- **Sidecar is positional only.** No risk of representational drift, because
  every byte of meaning lives in the lisp.

The widget is essentially a structured editor for one file format, with a
hot-reload trigger and an autocomplete popup. The "100x simpler" intuition
is right.
