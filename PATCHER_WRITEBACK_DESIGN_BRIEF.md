# Patcher Write-Back Design Brief

This document describes the write-back problem for the `eseqlisp` patcher.
It is meant as a handoff brief for a future design pass, not as an
implementation plan. The goal of that design pass should be a robust model for
turning patcher graph edits back into `dsp.lisp` while preserving source truth,
diagnostics, and future editing semantics.

## Context

The patcher currently opens a `dsp.lisp` file through an `eseqlisp` widget:

```lisp
(patcher
  :intent :instrument ; or :effect
  :path "instruments/.../dsp.lisp")
```

Current behavior is mostly projection plus in-memory editing:

- Parse `dsp.lisp` into a patch graph.
- Render nodes, ports, cables, macros, histories, params, and code islands.
- Allow panning, node selection, node dragging, node text editing, cable
  creation, cable selection, cable deletion, and cable endpoint reconnects in
  memory.
- Navigate into `defmacro` subpatch views.
- Never save or emit Lisp yet.

The Lisp file must remain the semantic source of truth. A future layout sidecar
may persist node positions, segmented cable presentation, and view state, but it
must not become a second semantic patch format.

## The Core Problem

Graph edits are not trivially equivalent to text edits.

The imported graph is a projection of Lisp expressions. Some graph nodes
correspond to top-level forms, some correspond to nested expressions, some are
synthetic projections of symbolic references, and some unsupported source forms
are intentionally represented as code islands. Write-back must know what each
editable object means in source before mutating it.

For example:

```lisp
(def sig (* (phasor pitch) 0.5))
(out sig 1)
```

This may project as:

- a `phasor` node,
- a `* 0.5` node,
- an `out 1` node,
- cables between them.

Moving a node is layout-only. Editing `* 0.5` to `+ 1` changes an expression.
Deleting the cable from `phasor` to `* 0.5` changes an argument position.
Deleting the `* 0.5` node changes the binding `sig` and possibly the output.
These operations require different source transformations.

## Design Goal

Design a write-back system that can:

- Represent source-backed graph objects and newly created graph objects with
  stable identities.
- Apply user graph operations as explicit edit operations.
- Determine whether the current edited graph can be emitted to valid DGenLisp.
- Emit deterministic Lisp for the representable subset.
- Leave unsupported source untouched unless the user explicitly edits through a
  supported path.
- Persist layout separately from semantics.
- Support root patches and macro subpatches with the same model.

The first implementation should prefer a small correct subset over broad
fragile behavior.

## Current In-Memory Edit Model

The current patcher has an in-memory edit state roughly shaped as:

- source node edits: node text and position overrides for nodes imported from
  Lisp.
- created node edits: new nodes created in the editor, with text and position.
- deleted source nodes.
- created connections.
- deleted source connections.
- selected nodes/cables and active drag state.

This is enough for interaction, but not enough for durable write-back because
it does not fully answer:

- Which source span or source expression owns each node?
- Is a node top-level, nested, synthetic, or projected from a reference?
- Which binding name should a newly created node receive?
- If a nested expression is edited, should it stay nested or be lifted to a
  top-level `def`?
- When a cable is deleted, what should replace that argument in source?
- When a cable is created, which argument slot is being assigned and how is
  the previous argument represented?
- How should write-back preserve source that the graph cannot represent?

## Important Source Shapes

The design should account for these source patterns.

### Top-Level Definitions

```lisp
(def pitch (in 1 @name pitch))
(def phase (phasor pitch))
(def sig (* phase 0.5))
```

Usually maps well to nodes and named edges. Write-back can rewrite each `def`
expression if node identity and binding identity are stable.

Open questions:

- Does every operator node need a binding name for write-back?
- Should anonymous editor-created nodes get generated binding names
  immediately or only on save?
- How are generated names kept stable across later saves?

### Nested Calls

```lisp
(def sig (* (triangle (phasor pitch)) gain))
```

Projection expands nested calls into separate nodes. Write-back must decide
whether to preserve nesting or normalize to top-level defs.

Possible policy:

- V1 write-back normalizes edited representable graphs to top-level defs.
- Unedited imported subtrees may preserve original source shape when possible.

This needs a clear decision. Preserving arbitrary nesting is harder but creates
smaller diffs. Normalizing is simpler but may rewrite large portions of a file.

### Inline Literal Arguments

The patcher displays trailing literals inline:

```lisp
(* signal 3)     ; displayed as [* 3], inlet 0 visible, inlet 1 hidden
(ap sig g 0.6)  ; displayed as [ap ? 0.6], inlets 0 and 1 visible
```

Invariant:

- Literal arguments can be embedded in node text.
- Connected argument slots render as visible inlets.
- `?` in node text means "keep this argument slot visible/connected here".
- Hidden literal ports still reserve their semantic argument positions.

Write-back must preserve semantic argument order even when visible ports are
compactly laid out.

### Params

```lisp
(param size @min 0 @max 3000 @default 300)
(def delayed (delay signal size))
```

Projection should be:

```text
[param size @min 0 @max 3000 @default 300] -> [delay]
```

The `delay` node should not display `size` inline. Write-back must preserve the
param declaration and emit symbol references where cables connect param nodes
to consumers.

Open questions:

- Can param nodes be renamed through node text?
- If a param node is renamed, should all symbol references update?
- How are param identity and display name separated?

### History

Source:

```lisp
(make-history h)
(def old (read-history h))
(write-history h sig)
```

Projection collapses this into one `history` node:

- reads come out of the history node,
- writes go into the history node,
- feedback-class cables distinguish write paths.

Write-back must expand the collapsed node back into `make-history`,
`read-history`, and `write-history` forms.

Open questions:

- Does a history node expose its source name in editable text or only as
  metadata?
- How should multiple reads and writes of the same history slot be ordered?
- Should writing to a history node generate a top-level `write-history` form
  or inline it in a dependent expression?

### Out Nodes

```lisp
(out sig 1 @name audio)
```

`out` nodes are sinks. Write-back for changed incoming cables should rewrite
the first argument of `out`, not create an extra binding unless needed.

Open questions:

- How should disconnected `out` nodes emit?
- Is a disconnected output an invalid graph state, a muted output, or a
  diagnostic-only unsaved state?

### Defmacro Subpatches

```lisp
(defmacro ap (sig g d)
  ...)

(def x (ap input 0.6 delay-time))
```

Macro definitions are navigable subpatches. Macro instances in the root graph
use the same inline literal rules as built-ins.

Inside a macro view:

- macro params display as `in 1`, `in 2`, etc.
- macro return values display as `out 1`, etc.
- the root breadcrumb identifies the active macro.

Write-back must support editing a macro body without confusing it with root
patch edits.

Open questions:

- Are macro params editable as names in the subpatch, or only through the
  `defmacro` header?
- How are multiple macro return values represented in source?
- If a macro instance is edited to a different arity, how does that relate to
  the macro definition's params?

### Destructuring Defs

```lisp
(def (re im) (fft signal))
```

Projection may show one multi-output node. Write-back must preserve or
reconstruct destructuring when a node has multiple named outputs.

Open questions:

- Are output names user-editable?
- If an output cable is deleted, does the output binding remain?
- How should generated output names be chosen for new multi-output nodes?

### Unsupported Forms / Code Islands

Unsupported forms render as code-island/error nodes. They exist to prevent
silent misrepresentation.

Write-back must not rewrite unsupported forms accidentally. A good invariant:

- If a source region projected to a code island, write-back preserves that
  exact source region unless the user explicitly edits that code island through
  a future source-aware editor.

Open questions:

- Can supported graph edits around a code island still be saved?
- What happens if a supported node depends on a binding defined inside a code
  island?

### Comments

The current parser skips comments. Comment-preserving write-back requires a
comment-preserving parser or source patching strategy.

V1 should probably treat comments as non-editable preserved trivia, or defer
comment preservation entirely if full-file normalization is chosen.

This decision is important because full-file pretty-printing without comment
support will delete comments.

## Source Identity Requirements

Write-back needs stable identities for at least:

- source forms,
- graph nodes,
- graph connections,
- macro definitions,
- macro-local graph nodes,
- generated bindings,
- layout sidecar entries.

Endpoint-derived cable IDs are currently sufficient for in-memory interaction,
but they are not enough long-term if duplicate equivalent connections or
source-span mapping matters. The design should decide whether cables become
first-class stable objects or remain derived from node arg references.

Potential identity strategies:

- Structural IDs from source paths, e.g. `root/def:sig/body/call:0`.
- Binding IDs from `(def name ...)`, e.g. `root/binding:sig`.
- Generated persistent IDs stored only in `dsp.layout.json`.
- Source annotations in Lisp, likely undesirable for V1.

The design should be explicit about which identities survive:

- source formatting changes,
- node moves,
- generated binding renames,
- macro edits,
- deleting and recreating a cable,
- hand-editing `dsp.lisp` outside the patcher.

## Edit Operations To Model

Avoid "mutating the graph and hoping emission works." The write-back design
should define explicit operations such as:

- `CreateNode { view, node_id, text, position }`
- `EditNodeText { view, node_id, old_text, new_text }`
- `MoveNode { view, node_id, position }`
- `DeleteNode { view, node_id }`
- `CreateConnection { view, from, to }`
- `DeleteConnection { view, connection_id }`
- `ReconnectConnectionEndpoint { view, connection_id, endpoint, new_port }`
- `OpenMacro { macro_name }`
- `EditMacroBody { macro_name, op }`

Each operation should state:

- whether it is semantic or layout-only,
- whether it can be saved to `dsp.lisp`,
- whether it only affects `dsp.layout.json`,
- which diagnostics it can produce,
- how it maps to source when saving.

## Emission Policy Choices

The design should choose one of these broad write-back strategies.

### Strategy A: Full Normalized Emit

For any save, emit a deterministic `dsp.lisp` from the graph for all
representable source.

Pros:

- Simple mental model.
- Easier to reason about graph correctness.
- Easier to implement once the graph IR is complete.

Cons:

- Large diffs.
- Comment preservation is hard.
- Unsupported code islands become difficult unless kept as raw source blocks.
- Hand-authored style is lost.

### Strategy B: Source Patch / Minimal Rewrite

Use source spans to rewrite only touched forms.

Pros:

- Better diffs.
- Can preserve formatting and comments around untouched forms.
- Safer around unsupported forms.

Cons:

- Requires span-aware parsing and source ownership.
- Nested expression edits are more complex.
- Harder to ensure global consistency after graph rewiring.

### Strategy C: Hybrid

Preserve untouched source regions, but normalize touched connected components.

Pros:

- Good compromise.
- Allows robust graph edits without rewriting entire file.
- Can preserve code islands and unrelated forms.

Cons:

- Needs clear component ownership rules.
- Can still create surprising diffs if graph components are large.

The recommended design pass should choose a strategy explicitly. If choosing
hybrid, define exactly what a "component" is in terms of bindings and source
forms.

## Suggested V1 Save Scope

Keep V1 intentionally narrow:

- Save layout-only node moves to `dsp.layout.json`.
- Save simple representable root graphs made of top-level `def`, `param`,
  `in`, `out`, operator calls, constants, and created/deleted/reconnected
  cables.
- Support macro body save only after root save is stable, unless macro editing
  falls out naturally from the same view-scoped model.
- Treat unsupported code islands as save blockers for semantic edits that
  would require moving through or rewriting them.
- Do not preserve comments until the parser/source patching strategy supports
  comment trivia.

Minimum vertical slice:

1. Open a simple `dsp.lisp`.
2. Move nodes.
3. Create a new operator node.
4. Cable it between an `in` and an `out`.
5. Save.
6. Reload.
7. The graph and compiled Lisp match the saved state.

## Diagnostics / Save Blocking

Write-back should distinguish:

- visual diagnostics that can remain unsaved,
- graph validation errors that block save,
- compiler diagnostics after save/compile.

Examples of save blockers:

- unknown operator after committing node text,
- disconnected required inlet,
- duplicate assignment to a single-input slot if the language cannot represent
  it,
- cycle that is not explicitly a supported history/feedback edge,
- edited component depends on unsupported code island whose binding cannot be
  resolved safely,
- generated binding name collision that cannot be resolved deterministically.

## Layout Sidecar

Layout should persist separately from Lisp:

```json
{
  "version": 1,
  "root": {
    "nodes": {
      "binding:pitch": { "x": 12.0, "y": 4.0 }
    },
    "pan": { "x": 0.0, "y": 0.0 }
  },
  "macros": {
    "ap": {
      "nodes": {
        "binding:delayed": { "x": 20.0, "y": 8.0 }
      }
    }
  }
}
```

The sidecar must not be required for compile correctness. If it is missing,
stale, or partially invalid, the patcher should fall back to auto-layout and
default cable slack. Cable entries are valid only when their stable endpoint
keys resolve to an existing source-projected connection; they must never create,
delete, or retarget graph connections.

Open questions:

- What stable node keys should the layout sidecar use?
- What stable connection keys should segmented cable layout use for anonymous
  expression nodes?
- Should generated unsaved node IDs ever be written to sidecar?
- Does pan/zoom persist globally or per macro view?

## Compiler Integration

Saving Lisp is separate from compiling it. The design should define:

- explicit save/apply action first,
- later optional hot-reload/debounced compile,
- parameter preservation rules after compile,
- failure behavior when emitted Lisp does not compile.

The patcher should not call compiler internals as a source of graph truth.
Compilation is validation/execution after the graph has emitted Lisp.

## Questions The Design Pass Should Answer

1. What is the durable source identity model for imported nodes and generated
   nodes?
2. Do we normalize all representable Lisp on save, patch source spans, or use a
   hybrid strategy?
3. What graph subset is saveable in the first implementation?
4. How do cable edits map to argument rewrites, especially with inline literals
   and `?` placeholders?
5. How are new nodes named in Lisp, and how are generated names kept stable?
6. How does node deletion behave when other nodes depend on its binding?
7. How are `param` renames and references handled?
8. How are collapsed history nodes expanded back to source?
9. How are macro views saved and scoped?
10. What is the exact save-blocking diagnostic model?
11. What goes in `dsp.layout.json`, and what must never go there?
12. What tests prove write-back is robust enough to build on?

## Expected Output Of The Design Pass

The next agent should produce:

- A concrete write-back architecture.
- Data structures for source ownership, edit operations, save diagnostics, and
  layout sidecar records.
- A chosen emission strategy with tradeoffs.
- A V1 implementation sequence.
- Test fixtures for root graph save/reload, inline args, params, histories,
  macros, unsupported code islands, and layout sidecar persistence.
- A clear list of intentionally unsupported cases for V1.

Do not start implementation until the source identity and emission policy are
settled. This is the part most likely to cause long-term pain if it is guessed.
