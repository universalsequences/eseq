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

A visual `history` node is not equivalent to a single expression. It has three
source roles:

- Declaration role: `(make-history name)`
- Read role: `(read-history name)` wherever the node's output is consumed
- Write role: `(write-history name value)` when the node's write input is fed

The emitter must never use a transient visual node id such as `created-6` as a
history source name. Source-backed histories keep their source symbol. Created
histories allocate a generated history name before any read or write form is
emitted.

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
- Emit write uses as `(write-history name value)` forms in the same source
  scope as the corresponding `make-history`.
- Preserve the original history source name for imported histories.
- Generate hidden history names only for newly created history nodes. Use the
  generated binding high-water policy with the `history` stem unless a more
  specific stem is introduced later.
- Generated history names are scoped exactly like generated `def` bindings:
  root histories use root counters, and macro-local histories use the owning
  macro scope's counters.
- A created history node must allocate one stable generated name and use that
  same name for its make, read, and write forms.
- Preserve original source order for existing make/read/write forms whenever
  possible.
- For newly emitted write forms, place the write after the forms needed to
  compute its value and inside the same source scope as the `make-history`.
- If a history write input is semantically present but disconnected, emit
  `__patcher_missing_input__` only through the same explicit incomplete-save
  path used for missing positional arguments.
- If a history node is read but has no write, still emit its `make-history` and
  reads. This represents the graph honestly, even if downstream validation later
  warns that the history is never written.
- If a history node is written but never read, still emit its `make-history` and
  write. Dead-code pruning is not part of V1 write-back.

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

Created feedforward history example:

```lisp
;; visual graph:
;; delayed -> history write input
;; history output -> consumer

(make-history history1)
(def delayed (delay node d_samples))
(write-history history1 delayed)
(- (read-history history1) (* g node))
```

Created macro-local histories are emitted inside the macro body and allocate
from that macro's history counter:

```lisp
(defmacro ap (sig g d_samples)
  (make-history history1)
  (def delayed (delay sig d_samples))
  (write-history history1 delayed)
  (read-history history1))
```

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
- Whether a history node with no reads or no writes should emit, warn, or be
  pruned.

Non-negotiable V1 history tests:

- Existing source history `h` round-trips as `h` for make, read, and write.
- Created history emits `(make-history history1)`, never
  `(make-history created-N)` or `(read-history created-N)`.
- Created history reads and writes use the same generated history name.
- Macro-local created histories allocate in the macro scope and emit inside
  `defmacro`.
- A history write cable emits a corresponding `write-history` form.
- A disconnected history write input emits `__patcher_missing_input__` only in
  incomplete-save mode.
- Multiple active writes to one history block normal V1 save.

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
- Cable presentation state for existing connections:
  - Whether the cable is segmented.
  - The patch-local row of the segmented cable's horizontal run.

Forbidden data:

- Graph semantics.
- Connection topology or any information that creates, deletes, or retargets a
  connection.
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
    },
    "cables": {
      "binding:pitch:0->binding:sig:0": {
        "segmented": true,
        "y": 6.5
      }
    }
  },
  "macros": {
    "ap": {
      "nodes": {
        "binding:delayed": { "x": 20.0, "y": 8.0 }
      },
      "cables": {
        "macro-param:input:0->binding:delayed:0": {
          "segmented": true,
          "y": 10.0
        }
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

Layout cable keys should identify source-projected endpoints using the same
stable node keys and explicit outlet/inlet indexes:

- `<from-node-key>:<outlet-index>-><to-node-key>:<inlet-index>`.
- Cable entries whose endpoints no longer resolve are stale layout data and
  must be ignored.
- Cable entries are applied only after the Lisp source has projected a matching
  connection. They must not synthesize missing connections.
- `y` is patch-local, not viewport-local, so panning changes the rendered row by
  the same amount as the connected nodes.

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

## Implementation Phases

The write-back implementation should proceed in deliberately small slices. Each
slice must be testable without relying on a real file save path.

### Phase 0: Completed Foundation

Status: implemented.

This phase establishes source ownership metadata on the projected patch graph.

Completed requirements:

- `SourceScopeId::{Root, Macro { name }}` exists.
- Source forms and source expressions are scoped.
- Nodes and connections carry source metadata.
- Root and macro scopes project separately.
- Params, macro params, histories, code islands, attributes, and nested
  expressions have source ownership tests.
- Current patcher behavior ignores source metadata for rendering and
  interaction.

This phase does not emit durable Lisp.

### Phase 1: Debug Preview Emitter

Status: implemented as a diagnostic aid, not as save logic.

The debug emitter prints a best-effort Lisp preview after semantic patcher
edits when `ESEQ_PATCHER_DEBUG_LISP` is enabled.

Completed requirements:

- Print preview after node text edits, cable edits, and node deletion.
- Gate preview behind an environment variable so release builds can opt in.
- Emit macro subpatch previews as `defmacro` forms.
- Map macro input nodes back to macro parameter names.
- Avoid printing synthetic macro return `out` when the source return expression
  is already emitted.

Known limitations:

- It is not complete-file write-back.
- It is allowed to be approximate for unsupported source.
- It does not allocate durable generated bindings.
- It does not implement complete history make/read/write emission.
- It must not become the save path by incremental patching.

### Phase 2: In-Memory Normalized Write-Back Emitter

Status: implemented.

Goal: produce complete save-ready Lisp text from source text plus patcher edit
state, but do not write files yet.

Proposed module:

```rust
crates/eseqlisp/src/widget_render/patcher/writeback.rs
```

Initial API:

```rust
pub(super) fn emit_patch_writeback(
    source: &str,
    intent: PatcherIntent,
    interaction_state: &PatcherInteractionState,
) -> Result<String, WriteBackError>;
```

Responsibilities:

- Parse the original source.
- Re-project into the source-aware patch model.
- Apply the current patcher interaction state.
- Emit a complete normalized Lisp document.
- Preserve unsupported/code-island source forms unchanged unless they intersect
  a semantic edit.
- Return structured save blockers instead of silently dropping unsupported
  semantics.

Non-goals:

- No file writes.
- No editor save command.
- No `dsp.layout.json` writes.
- No byte-span surgical patching.
- No comment/trivia preservation beyond preserving untouched raw code islands
  when possible.

Phase 2 should start with exact-output tests for unchanged and simple edited
source:

- Unchanged root patch emits complete normalized Lisp.
- Unchanged macro emits complete normalized `defmacro`.
- Node text edit rewrites the owning expression.
- Macro parameter inputs emit parameter names, not `(in N)`.
- Synthetic macro return `out` does not appear in emitted Lisp.

Completed requirements:

- Added an in-memory normalized write-back emitter in
  `crates/eseqlisp/src/widget_render/patcher/writeback.rs`.
- The emitter parses the original source, re-projects source ownership metadata,
  applies source-backed node text edits, and emits complete normalized Lisp.
- Root and macro scopes are emitted from the parsed source tree rather than from
  visual graph order.
- Simple source-backed root, macro, and nested node text edits replace the owned
  source subtree while preserving same-shape nested structure.
- Unsupported Phase 3-5 semantics return structured blockers instead of
  silently emitting approximate Lisp.
- No file save path, layout sidecar write, generated binding allocation, or
  durable history emission was added.

### Phase 3: History-Aware Write-Back

Status: implemented.

Goal: make history a first-class normalized-emission entity.

Responsibilities:

- Resolve each source-backed visual history node to a stable history identity.
- Allocate generated history names for created history nodes.
- Emit exactly one `make-history` per live history node.
- Emit `read-history` expressions at each consumer site.
- Emit `write-history` forms for history write inputs.
- Keep make/read/write names consistent.
- Scope generated history names to root or the owning macro body.
- Block normal save for multiple active writes to one history.
- Block cross-scope history reads/writes.

Tests:

- Existing feedforward history round-trips:

  ```lisp
  (make-history h)
  (def delta (- sig (read-history h)))
  (write-history h sig)
  ```

- Existing feedback history round-trips:

  ```lisp
  (make-history h)
  (write-history h (mix sig (read-history h) alpha))
  ```

- Created feedforward history emits:

  ```lisp
  (make-history history1)
  ...
  (write-history history1 value)
  ...
  (read-history history1)
  ```

- Macro-local created histories emit inside `defmacro`.
- Visual ids such as `created-5` and `created-6` never appear as history names.
- Missing write input uses `__patcher_missing_input__` only through incomplete
  save.

This phase should not be implemented as a debug emitter patch. It belongs in
the real normalized write-back emitter because generated names and history
ordering are durable semantics.

Completed requirements:

- Existing source-backed feedforward and feedback histories round-trip through
  the real normalized write-back emitter.
- Created root and macro-local history nodes allocate scoped generated names
  such as `history1`, never visual ids such as `created-N`.
- Created history read and write edges use the same generated history name for
  `make-history`, `read-history`, and `write-history`.
- Macro-local created histories emit inside the owning `defmacro`, with writes
  inserted before the macro return expression.
- Multiple active writes to the same history return a structured blocker.
- History edits that would require generated value bindings remain blocked
  until generated binding allocation is implemented.

### Phase 4: Generated Binding Allocation

Goal: allocate deterministic source names for created or lifted value nodes.

Responsibilities:

- Scan existing root and macro-local names.
- Build per-scope high-water counters by sanitized stem.
- Allocate generated `def` bindings only when a node requires a source-level
  name.
- Store allocated names in the edit/write-back model for the duration of the
  emit so repeated references use the same name.
- Avoid collisions with params, macro params, histories, macro names, and
  existing defs in the same scope.

Tests:

- Existing `phasor1` causes the next generated phasor binding to be `phasor2`.
- Deleted generated names are not reused during the same session.
- Macro-local generated names do not collide with root generated names.
- Shared created node emits one generated `def` and multiple references.

### Phase 5: Cable and Node Semantic Write-Back

Goal: cover the core patch editing operations as saveable normalized output.

Responsibilities:

- Cable creation rewrites the destination semantic arg slot.
- Cable deletion emits `__patcher_missing_input__` when the required input is
  truly missing.
- Source node deletion removes the owned top-level form or replaces the owned
  nested expression with a missing sentinel where necessary.
- Created node deletion removes the temporary edit.
- Deleted nodes remove or rewrite incident connections deterministically.
- Attributes remain inline and never become cable-addressable.

Tests:

- Cable create updates the intended semantic arg index.
- Raw child indexes remain correct when attributes are present.
- Cable delete in root emits the missing-input sentinel.
- Cable delete in macro emits the missing-input sentinel inside `defmacro`.
- Deleting a source-backed top-level node removes that form.
- Deleting a nested producer preserves downstream source with an explicit
  missing input or blocks save when no unambiguous representation exists.

### Phase 6: Param and Macro Parameter Rename

Goal: rename binding identities, not text occurrences.

Responsibilities:

- Param node text rename updates the declaration.
- All resolved references to that binding emit the new name.
- Unresolved same-text symbols are not rewritten.
- Macro parameter rename updates the `defmacro` header and resolved references
  inside the macro scope, if macro parameter rename is enabled.
- Collisions block save.

Tests:

- Param rename updates all resolved root references.
- Macro parameter rename updates header and macro-local references.
- Unresolved symbols with the old text remain unchanged.
- Code islands that may reference the old name produce a diagnostic or blocker.

### Phase 7: File Save Wiring

Goal: connect the normalized emitter to an explicit save operation.

Prerequisites:

- Phase 2 complete.
- Phase 3 complete.
- Save blockers return structured diagnostics.

Responsibilities:

- Add a save command/path that calls `emit_patch_writeback`.
- Write the emitted Lisp to the source file only after successful emission.
- Block normal save for compile-invalid sentinel output.
- Add an explicit incomplete-save path if desired.
- Keep debug preview separate from save behavior.

Tests:

- Save writes expected normalized source to a temp file.
- Save refuses unresolved blockers.
- Save does not modify file on error.

### Phase 8: Layout Sidecar Persistence

Goal: persist non-semantic layout state.

Responsibilities:

- Write `dsp.layout.json` with root and macro node positions.
- Write segmented cable presentation state for root and macro connections.
- Use semantic layout keys when available.
- Migrate temporary created keys to emitted binding/history keys after save.
- Ignore stale sidecar entries safely.

Tests:

- Root node positions persist and reload.
- Macro node positions persist under the macro namespace.
- Segmented cable settings persist and reload under the correct scope.
- Stale cable entries do not create, delete, or retarget connections.
- Stale sidecar entries do not affect parsing or compile semantics.

## Open Questions To Continue

The following design questions remain unsettled and should be resolved before
implementation proceeds beyond source identity:

1. Which tests define the minimum safe vertical slice?
