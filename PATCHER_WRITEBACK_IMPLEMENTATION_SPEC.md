# Patcher Write-Back Implementation Spec

This document turns the write-back design brief into concrete implementation
decisions for durable, non-destructive editing of DGenLisp through the patcher.
It is intentionally incremental: decisions that are settled are specified here;
unsettled choices remain explicit open questions.

The core principle is that `dsp.lisp` remains the semantic source of truth. The
patcher is an editable projection of Lisp, not a second semantic patch format.

## First Principles

The patcher must support two related but different workflows:

- Open hand-written Lisp, edit it visually, and preserve as much source
  structure as possible.
- Create a patch from scratch visually and emit deterministic, legible Lisp.

These workflows require source ownership metadata. A rendered node is not enough
to decide what should be rewritten on save, because one Lisp expression can
project into several visual nodes and one visual node may represent several
source forms.

For example:

```lisp
(def result
  (phasor
    (* 25 (param freq @min 1 @max 100))
    (rampToTrig xyz)))
```

This may project into separate `phasor`, `*`, `25`, `param`, and `rampToTrig`
nodes. Each projected node must retain the source expression that produced it.
The `(rampToTrig xyz)` node is not top-level; it is owned by a nested expression
inside the value expression of `result`.

## Source Identity

### Source Form Identity

Every parsed top-level form receives a stable source-form identity for the
current parse.

```rust
struct SourceFormId {
    index: usize,
}
```

`SourceFormId` is parse-local. It is sufficient for computing edits against the
currently loaded source. It is not a durable cross-save identity by itself.

Durable identities for layout and generated bindings are separate concerns.

### Expression Paths

Every expression inside a top-level form is addressable by a structural path
from that top-level form root.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceExprId {
    form_id: SourceFormId,
    path: ExprPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExprPath(Vec<ExprPathSegment>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ExprPathSegment {
    ListItem(usize),
}
```

The top-level form itself has an empty path. Child indexes are raw AST child
indexes, not semantic argument indexes.

Example:

```lisp
(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))
```

Assuming this is top-level form `0`:

```text
SourceExprId { form: 0, path: [] }       ; whole def
SourceExprId { form: 0, path: [2] }      ; (phasor ...)
SourceExprId { form: 0, path: [2, 1] }   ; (* 25 ...)
SourceExprId { form: 0, path: [2, 1, 2] }; (param ...)
SourceExprId { form: 0, path: [2, 2] }   ; (rampToTrig xyz)
```

This is the main substrate for source ownership. Byte spans are still needed
for minimal source patching and comment preservation, but tree paths are the
semantic identity.

## Positional Arguments vs Attributes

Lisp call children must be split into two different source concepts:

- Positional arguments, which may become graph inputs and cable endpoints.
- Attribute key-value pairs, which remain inline node metadata.

Attributes are not graph inputs. They should never become cable endpoints or
separate patcher nodes in V1.

```rust
struct CallSourceShape {
    call: SourceExprId,
    positional_args: Vec<ArgSource>,
    attributes: Vec<AttributeSource>,
}

struct ArgSource {
    semantic_index: usize,
    item_index: usize,
    expr: SourceExprId,
}

struct AttributeSource {
    key_item_index: usize,
    value_item_index: usize,
    key: String,
    value: SourceExprId,
}
```

For:

```lisp
(param freq @min 1 @max 100 @default 25)
```

the raw list children are:

```text
0 param
1 freq
2 @min
3 1
4 @max
5 100
6 @default
7 25
```

The semantic positional arguments are:

```text
arg 0 -> raw item 1 -> freq
```

The attributes are:

```text
@min     -> raw items 2,3
@max     -> raw items 4,5
@default -> raw items 6,7
```

Cable edits operate only on `ArgSource`. Node-text edits may rewrite operator
text, inline positional literals or placeholders, and attributes.

## Node Source Ownership

Every source-backed `PatchNode` must carry ownership metadata separate from its
visual id.

```rust
struct NodeSource {
    owner: SourceOwner,
    expr: Option<SourceExprId>,
    call_shape: Option<CallSourceShape>,
}

enum SourceOwner {
    TopLevelForm {
        form_id: SourceFormId,
    },
    BindingValue {
        form_id: SourceFormId,
        binding: BindingTarget,
        value_path: ExprPath,
    },
    NestedExpr {
        expr: SourceExprId,
    },
    ArgumentSlot {
        call: SourceExprId,
        arg: ArgSource,
    },
    SymbolReference {
        call: SourceExprId,
        arg: ArgSource,
        symbol: String,
        resolved_binding: Option<BindingId>,
    },
    Compound {
        parts: Vec<SourceOwner>,
    },
    CodeIsland {
        form_id: SourceFormId,
    },
    Created {
        created_id: String,
        generated_binding: Option<BindingId>,
    },
}

enum BindingTarget {
    Symbol(String),
    Destructuring(Vec<String>),
}

struct BindingId {
    name: String,
}
```

The owner answers: if this node is semantically edited, what source region is
the patcher authorized to rewrite?

Important cases:

- A top-level `(def sig (phasor pitch))` projects a `phasor` node owned by the
  binding value for `sig`.
- A nested `(* 25 freq)` projects a `*` node owned by a nested expression.
- A literal `25` node created during projection is owned by a positional
  argument slot, not by an independent top-level form.
- A symbol reference such as `pitch` in `(phasor pitch)` is owned by the
  consumer argument slot. The producer node owns the binding that defines
  `pitch`.
- A history node may be compound because it can represent `make-history`,
  `read-history`, and `write-history` source forms.
- A code island owns raw unsupported source and blocks semantic rewrites through
  that region.

## Connection Source Ownership

Connections must be mapped to destination argument slots.

Endpoint-derived connection ids are acceptable for interaction, but write-back
needs the destination slot owner:

```rust
struct ConnectionSource {
    from_expr: Option<SourceExprId>,
    to_call: SourceExprId,
    to_arg: ArgSource,
    previous_arg: SourceArgValue,
}

enum SourceArgValue {
    Literal(SourceExprId),
    SymbolReference {
        expr: SourceExprId,
        symbol: String,
        resolved_binding: Option<BindingId>,
    },
    NestedExpression(SourceExprId),
    Missing,
}
```

When a cable is deleted, write-back consults `previous_arg` and the destination
node policy to decide whether the source can be restored, replaced with a
placeholder, or must block save.

When a cable is created, write-back rewrites `to_arg` to reference the producer
binding or generated binding.

## Parser Requirements

The current patcher parser produces source expressions but does not preserve
enough metadata for write-back.

Write-back requires a source-aware parse layer that provides:

- Top-level form indexes.
- `ExprPath` for every expression.
- Byte spans for expressions and forms.
- Raw list child indexes.
- Comment/trivia preservation strategy, if minimal source patching is used.

`ExprPath` should be introduced before byte-span source patching. It is needed
for correct graph ownership even if V1 emits normalized source.

## Emission Policy

V1 uses structure-preserving normalized emit.

This means save does not surgically patch byte ranges in the original source.
Instead, save builds an updated source tree from the patcher's source ownership
model and emits deterministic Lisp from that tree.

Normalized emit does not mean flattening everything into top-level temporary
bindings. The emitter should preserve nested structure whenever the edited graph
can still be represented by the original source tree shape.

For example, this hand-written structure:

```lisp
(def result
  (phasor
    (* 25 (param freq @min 1 @max 100))
    (rampToTrig xyz)))
```

should remain nested after unrelated or same-shape edits:

```lisp
(def result
  (phasor
    (* 25 (param freq @min 1 @max 100))
    (trig xyz)))
```

The emitter may introduce generated `def` bindings only when a graph edit
cannot be represented cleanly inside the existing nested tree. Generated
bindings are a semantic fallback for genuinely new graph structure, not the
default representation for every intermediate node.

Rationale:

- One deterministic emit path is simpler to reason about than mixing surgical
  source patches with component regeneration.
- Preserving AST shape through `SourceExprId` and `ExprPath` keeps the important
  part of hand-authored Lisp intact: nesting, expression ownership, and logical
  grouping.
- Large textual diffs are less important than preserving the user's meaningful
  source structure.
- V1 should avoid comment/trivia-preserving patch complexity until the AST and
  graph ownership model are correct.

Implications:

- Whitespace formatting is deterministic and may change on save.
- Comments are not preserved in V1 unless the parser models them as trivia.
- Unsupported top-level source must be carried through as raw preserved forms or
  block semantic save if it intersects a dirty emitted component.
- The graph cannot be the only source of truth for emission; it must be paired
  with the source-owned AST so nested source shape can survive.

## Nested Expression Edit Policy

V1 preserves nested expressions by default for local expression edits.

Editing the text or inline attributes of a source-backed nested node rewrites
that node's owned expression in place in the source tree. This is the simplest
correct behavior because `ExprPath` already identifies the exact subtree to
replace.

Example:

```lisp
(def result
  (phasor
    (* 25 (param freq @min 1 @max 100))
    (rampToTrig xyz)))
```

If the user edits the `rampToTrig` node text to `trig`, V1 emits:

```lisp
(def result
  (phasor
    (* 25 (param freq @min 1 @max 100))
    (trig xyz)))
```

It should not emit:

```lisp
(def generated1 (trig xyz))
(def result
  (phasor
    (* 25 (param freq @min 1 @max 100))
    generated1))
```

Blanket lifting of every touched nested expression is not the simplest V1
policy. It requires generated binding names even for edits that are already
representable as a direct subtree replacement, and it degrades source shape
without buying correctness.

V1 lifts nested expressions only when the graph topology can no longer be
represented cleanly by the existing expression tree.

Lift triggers:

- A nested node is used by more than one consumer.
- A graph edit needs to connect a value into a source location that cannot be
  represented as an inline subtree without duplicating nodes.
- A newly created node must be referenced by multiple downstream nodes.
- A cable edit would require moving an expression across ownership boundaries
  in a way that makes the original tree path ambiguous.

When lifting is required, lift the minimum affected expression, not the entire
surrounding component. The original nested expression is replaced by a symbol
reference to the generated binding.

Example:

```lisp
(def result (phasor (trig xyz)))
```

If `(trig xyz)` is rewired so two consumers use it, V1 may emit:

```lisp
(def generated1 (trig xyz))
(def result (phasor generated1))
(def other (some-op generated1))
```

Generated binding policy is specified separately. Lifting is allowed only after
the generated binding can be named deterministically without collision.

## Deleted Cable Policy

Deleting a cable can produce an intentionally incomplete graph. The in-memory
patcher must support this state, but the emitted Lisp must make the missing
argument explicit instead of silently choosing a default.

V1 represents a disconnected required positional argument with a reserved
sentinel symbol:

```lisp
__patcher_missing_input__
```

This symbol is not valid DGenLisp semantics. Emitted Lisp containing it is a
diagnostic artifact that must not be treated as compile-ready DSP code.

Example source:

```lisp
(out (phasor 50) 1)
```

If the cable from `(phasor 50)` to `out` is deleted, the source tree has lost
the first positional argument of `out`. V1 may emit:

```lisp
(def generated1 (phasor 50))
(out __patcher_missing_input__ 1)
```

The `phasor` expression is lifted only if necessary to preserve the now
disconnected node as an editable graph object. If the deleted producer node has
no remaining semantic owner and no reason to survive, deletion policy may remove
it instead, but it must not silently disappear as a side effect of cable
deletion unless the user explicitly deletes the node.

Replacement rules:

- If a cable deletion reveals an original inline literal or symbol that was
  merely hidden by a temporary in-memory connection edit, restore that previous
  source argument.
- If the source argument was a connected expression or symbol reference and the
  user deletes the cable, replace that positional argument with
  `__patcher_missing_input__`.
- If the destination argument is optional according to the operator's source
  contract, remove the argument only when doing so preserves valid call shape.
- Attribute values are never replaced by the missing-input sentinel because
  attributes are not cable endpoints.

Save behavior:

- Saving layout-only edits is allowed even when missing-input sentinels exist
  in memory.
- Saving semantic edits may write `__patcher_missing_input__` only through an
  explicit "save incomplete patch" path.
- The normal compile/apply path must block while any required input contains
  `__patcher_missing_input__`.
- Diagnostics must identify the destination node and semantic argument index
  for each missing input.

## Generated Binding Policy

V1 does not expose user-editable binding names in the patch editor.

All source-level bindings required for newly created nodes or lifted nested
expressions are generated automatically. Node text remains focused on the
operator and inline arguments/attributes, not on binding syntax such as
`lfo = phasor` or `lfo=phasor`.

Rationale:

- Binding-name UI adds a second editing language inside node labels.
- Generated names are enough for V1 save/reload correctness.
- User-authored naming can be added later as an explicit node property or
  source-aware rename operation, not as an overloaded text convention.

Generated binding names use the operator as a readable base plus a monotonic
integer suffix:

```text
phasor1
phasor2
mul1
delay1
generated1
```

Name generation rules:

- Sanitize the operator into a valid symbol stem.
- Prefer the sanitized operator stem.
- Use `generated` when the operator has no usable symbol stem.
- Append a positive monotonic integer suffix.
- Never collide with an existing binding, param name, macro name, history name,
  or another generated binding in the same source scope.

Allocation timing:

- Creating a node visually does not immediately allocate a binding.
- A binding is allocated when the node first needs a source-level name.
- A node needs a source-level name when it is emitted as a top-level `def`, when
  it is lifted out of a nested expression, or when multiple consumers require a
  shared value.
- A node that can remain inline in the owning expression tree does not need a
  generated binding.

The monotonic counter is scoped to the source file for the root patch and to
the macro body for macro subpatches. Macro scoping is specified separately.

Examples:

```lisp
(out (phasor 50) 1)
```

If `(phasor 50)` remains inline, no generated binding is allocated.

If that value must be preserved as a disconnected editable node or shared by
multiple consumers, V1 may emit:

```lisp
(def phasor1 (phasor 50))
(out phasor1 1)
```

If another generated phasor binding is later needed, it receives `phasor2`
unless that name is already occupied, in which case the counter advances until
it finds a free name.

## Generated Binding Stability

Generated binding names use per-stem high-water marks, not a free list.

When opening existing source, the patcher scans all bindings in the relevant
scope and initializes the next generated suffix for each stem to one greater
than the largest existing generated suffix for that stem.

Example existing generated bindings:

```lisp
(def phasor1 ...)
(def rampToTrig1 ...)
(def rampToTrig2 ...)
(def mult1 ...)
(def mult2 ...)
```

The next generated bindings are:

```text
phasor    -> phasor2
rampToTrig -> rampToTrig3
mult      -> mult3
```

If `phasor1` is later deleted, the next generated phasor binding is still
`phasor3` if `phasor2` has already been allocated in the editing session. The
patcher does not reuse `phasor1` merely because it became available.

Rationale:

- High-water marks are deterministic and easy to reason about.
- Reusing names can make unrelated edits appear to retarget old layout,
  diagnostics, or user mental models.
- Generated symbols should behave like stable identities once emitted.
- Avoiding a free list keeps save/reload behavior simple.

The high-water mark must account for:

- Existing generated bindings parsed from source.
- Generated bindings allocated during the current editing session.
- Generated bindings in incomplete saves that contain
  `__patcher_missing_input__`.

The high-water mark is scoped:

- Root patch: one counter map for the file-level source scope.
- Macro body: one counter map per macro source scope.

V1 determines whether a binding is generated by matching the generated naming
scheme for a known operator stem. This means a hand-authored binding named
`phasor7` will reserve that suffix. That is acceptable in V1 because avoiding
collisions is more important than distinguishing authored and generated names.

## Param Rename Policy

Param nodes have stable identity separate from their current source name.

Renaming a param through node text is allowed in V1, and it updates all resolved
references to that param in the same source scope.

Example:

```lisp
(param freq @min 1 @max 100)
(out (phasor freq) 1)
```

If the param node is renamed from `freq` to `frequency`, V1 emits:

```lisp
(param frequency @min 1 @max 100)
(out (phasor frequency) 1)
```

The visual graph keeps its cables because the node identity did not change; only
the source symbol changed.

Required identity model:

```rust
struct ParamId {
    scope: SourceScopeId,
    original_name: String,
    defining_expr: SourceExprId,
}

struct ParamSource {
    id: ParamId,
    current_name: String,
    declaration: SourceExprId,
}
```

`SymbolReference` already carries enough information to participate in this:

```rust
SymbolReference {
    call: SourceExprId,
    arg: ArgSource,
    symbol: String,
    resolved_binding: Option<BindingId>,
}
```

For param references, `resolved_binding` must point to the `ParamId`/binding
represented by the param declaration, not merely store the symbol text. During
emit, any symbol reference resolved to a renamed param is emitted with the
param's current name.

Rename rules:

- Rename is scoped to the current root patch or macro body.
- The new name must be a valid symbol.
- The new name must not collide with another param, binding, macro parameter,
  generated binding, history name, or macro name in the same scope.
- All resolved references to the param are updated on emit.
- Unresolved symbols that happen to have the same text are not rewritten.
- Code islands are not rewritten. If a code island may reference the old param
  name, emit a diagnostic warning or block compile/apply depending on how
  conservative V1 wants to be.

This means source ownership is sufficient only if projection resolves symbol
references to binding identities. A text-only search/replace is not acceptable.

## History Node Policy

History nodes have stable identity separate from their source symbol name.

The patcher presents a single `history` node for the source-level group made of:

- one `(make-history name)` declaration,
- zero or more `(read-history name)` expressions,
- zero or more `(write-history name value)` forms.

The user should not need to know or edit the source symbol used by
`make-history`. As with generated bindings, history source names are an emission
detail in V1.

Required identity model:

```rust
struct HistoryId {
    scope: SourceScopeId,
    original_name: String,
    defining_expr: SourceExprId,
}

struct HistorySource {
    id: HistoryId,
    current_name: String,
    make_form: SourceExprId,
    reads: Vec<HistoryReadSource>,
    writes: Vec<HistoryWriteSource>,
}

struct HistoryReadSource {
    expr: SourceExprId,
}

struct HistoryWriteSource {
    form: SourceExprId,
    value_arg: ArgSource,
}
```

Projection rules:

- `(make-history h)` creates the `HistoryId`.
- `(read-history h)` resolves to that `HistoryId` and projects as an output from
  the single visual history node.
- `(write-history h value)` resolves to that `HistoryId` and projects as an
  input to the same visual history node.
- The visual node's source owner is `SourceOwner::Compound` because no single
  expression owns the whole node.
- Unresolved reads or writes become code islands or diagnostics; they must not
  silently create independent histories.

Emission rules:

- Emit exactly one `(make-history name)` for each history node that survives.
- Emit read uses as `(read-history name)` at each consumer argument where the
  history output is connected.
- Emit write uses as top-level `(write-history name value)` forms.
- Preserve the original history source name for imported histories.
- Generate hidden history names only for newly created history nodes. Use the
  generated binding high-water policy with the `history` stem unless a more
  specific stem is introduced later.
- Preserve original source order for existing make/read/write forms whenever
  possible.
- For newly emitted write forms, place the write after the forms needed to
  compute its value and inside the same source scope as the `make-history`.

Examples:

Feedforward history:

```lisp
(make-history h)
(def delta (- sig (read-history h)))
(write-history h sig)
```

Feedback history:

```lisp
(make-history h)
(write-history h (mix sig (read-history h) alpha))
```

Both use the same visual model: the history output is a read value, and the
history input is the value written for the next sample/block.

V1 save scope:

- Support imported histories with one `make-history` and at most one active
  write edge.
- Support any number of read consumers.
- Support preserving and reconnecting the single write edge.
- Block save for multiple writes to the same history until ordering and merge
  semantics are explicitly designed.
- Block save when a history read/write crosses source scopes.
- Block save when the `make-history` source form is inside a code island or
  otherwise not source-owned.

Open history decisions after V1:

- Whether multiple writes are legal and how they are ordered.
- Whether history source names should become user-editable.
- Whether a disconnected history write input is allowed, removed, or represented
  with `__patcher_missing_input__`.
- Whether a history node with no reads or no writes should emit, warn, or be
  pruned.

## Macro Subpatch Policy

Each `defmacro` body is a separate source scope and patch namespace.

V1 must not treat an entire macro definition as one massive nested expression.
Instead, the macro header owns the macro name and parameter list, and the macro
body is projected as a scoped patch with its own source forms.

Source scope model:

```rust
enum SourceScopeId {
    Root,
    Macro { name: String },
}

struct ScopedSourceFormId {
    scope: SourceScopeId,
    index: usize,
}

struct ScopedSourceExprId {
    form_id: ScopedSourceFormId,
    path: ExprPath,
}
```

The earlier `SourceFormId`/`SourceExprId` types should either be extended with
`SourceScopeId` or replaced by scoped forms before macro write-back is
implemented.

Projection rules:

- The root scope contains top-level forms outside macro bodies and macro
  definition headers.
- Each macro body receives its own source scope, binding table, generated-name
  counters, param identities, history identities, diagnostics, and layout
  namespace.
- Macro parameters project as source-owned input nodes in the macro scope.
- The macro return expression projects as an output node in the macro scope.
- Macro instances in the root scope are ordinary call nodes whose operator is
  the macro name and whose arity comes from the macro header.

Emission rules:

- Saving the root scope emits root forms and preserves/re-emits macro
  definitions.
- Saving a macro subpatch rewrites only that macro's body scope and header
  details explicitly owned by the macro edit operation.
- Macro body forms are emitted inside the owning `defmacro`.
- Generated bindings inside a macro use that macro's generated-name counters,
  not the root counters.
- Param renames inside a macro update references to macro parameters inside that
  macro scope only.
- Root params and macro params are distinct identities even when they share the
  same symbol text.

V1 macro parameter policy:

- Macro parameter count and order are fixed by the `defmacro` header.
- Macro input nodes are not freely creatable/deletable in V1.
- Renaming macro parameters is allowed only if implemented as the same
  binding-identity rename used for params, scoped to the macro body and header.
  If that is not implemented, macro parameter names are read-only.
- Editing a macro instance's node text may change inline literal arguments, but
  changing macro arity is a save blocker unless the macro definition header is
  explicitly edited through a supported path.

V1 macro return policy:

- A single return value is supported first.
- The final macro body expression owns the return value unless the source uses
  an explicit supported output representation.
- Multiple return values are a save blocker until destructuring/multi-output
  macro semantics are designed.

Layout sidecar scope:

- Root layout is stored under the root namespace.
- Macro layout is stored under `macros[macro_name]`.
- Node ids only need to be unique inside their source scope.

Save blockers:

- Editing a macro body whose source ownership cannot be scoped cleanly.
- Cross-scope cables between root and macro body views.
- Macro arity changes without a supported header edit.
- Multiple macro return values.
- Macro body code islands touched by semantic graph edits.

## Layout Sidecar Policy

`dsp.layout.json` is a non-semantic sidecar.

V1 stores only UI/layout state that cannot be represented in Lisp and is not
required for compile correctness.

Allowed V1 data:

- Node positions.

Forbidden data:

- Graph semantics.
- Connections.
- Node operators.
- Node arguments.
- Param values.
- Generated binding counters.
- Source ownership.
- Whether a symbol was user-authored or generated.
- Pan.
- Zoom.
- Selection.
- Last active view.
- Anything required to compile the patch correctly.

If the sidecar is missing, stale, partially invalid, or references nodes that no
longer exist, the patcher must fall back to auto-layout for those entries. The
Lisp source remains the only semantic source of truth.

Sidecar shape:

```json
{
  "version": 1,
  "root": {
    "nodes": {
      "binding:pitch": { "x": 12.0, "y": 4.0 },
      "expr:0/2/1": { "x": 24.0, "y": 8.0 }
    }
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

Layout node keys should prefer semantic stable keys when available:

- `binding:<name>` for top-level binding-backed nodes.
- `param:<name>` for params.
- `history:<name>` for histories.
- `macro-param:<name>` for macro parameters inside macro scope.
- `expr:<path>` for nested expression nodes without a binding.
- `created:<id>` only for unsaved editor-created nodes; these should not be
  persisted after save unless they have been assigned a stable emitted identity.

When a generated binding is emitted, its layout key should migrate from the
temporary created key to the emitted binding key.

## Projection Requirements

Projection must pass `SourceExprId` through all recursive calls that turn Lisp
expressions into graph nodes.

Specifically:

- `project_top_level` starts with `SourceFormId` and root `ExprPath`.
- `project_def` records the binding target and the value expression path.
- `project_value` receives the current `SourceExprId`.
- `project_call` creates `CallSourceShape` from raw list children.
- Nested calls receive nested `SourceExprId` values.
- Constants projected as separate nodes keep the owning argument slot.
- Symbol references keep both the consumer argument slot and resolved producer
  binding, when available.
- Param declarations get stable `ParamId` identities, and param symbol
  references resolve to those identities.
- History declarations get stable `HistoryId` identities, and read/write forms
  resolve to those identities.
- Macro bodies are projected under `SourceScopeId::Macro`, with source forms,
  bindings, generated names, params, histories, and layout isolated from root.

`PatchNode` should gain a source field:

```rust
pub struct PatchNode {
    pub id: String,
    pub op: String,
    pub kind: NodeKind,
    pub label: String,
    pub args: Vec<ArgValue>,
    pub outputs: Vec<String>,
    pub position: (f32, f32),
    pub diagnostic: Option<String>,
    pub source: Option<NodeSource>,
}
```

If exposing this publicly is undesirable, split the render graph from an
internal write-back graph instead of hiding ownership in ad hoc maps.

## V1 Save Boundaries

V1 should save only cases whose ownership is unambiguous.

Saveable:

- Layout-only node movement to a layout sidecar.
- Root graphs made from top-level `def`, `param`, `in`, `out`, constants, known
  operator calls, inline positional literals, and created/deleted/reconnected
  cables.
- Histories with one `make-history`, any number of reads, and at most one
  active write.
- Nested expressions whose updated source tree can be emitted
  deterministically while preserving the existing nested structure where
  possible.
- Attribute edits that remain inline in the owning call expression.

Save blockers:

- Edited code islands.
- Semantic edits whose `SourceOwner` is missing or ambiguous.
- Cable edits targeting an attribute instead of a positional arg.
- Cable edits requiring a binding name that cannot be generated without
  collision.
- Unknown operators.
- Disconnected required inputs in the normal compile/apply path. They may be
  emitted only as `__patcher_missing_input__` through an explicit incomplete
  save path.
- Compound owners whose emitter is not specified.
- Histories with multiple active writes, cross-scope reads/writes, or
  unsupported source ownership.

## Open Questions To Continue

The following design questions remain unsettled and should be resolved before
implementation proceeds beyond source identity:

1. Which tests define the minimum safe vertical slice?
