# Patcher agentic connect

Wire a selected node into the surrounding patch from a cmd+k-style bubble:
"connect it". The agent proposes a **connection plan**; the host validates and
applies it with the same primitives the mouse and keyboard drive.

Motivating case: cmd+k produces a `vibraphone` macro instance at root with six
inlets and no cables. By hand you would wire the patch's trigger input to the
macro's `trig` inlet, its pitch input to `freq`, and give `decay` a value to get
sound. All of that is inferable from the macro's parameter names plus what is
already on the canvas.

## 1. Non-goals

- **Never emits or rewrites source.** Unlike `resolve_agentic_bubble` and
  `resolve_agentic_bubble_macro_edit` (§4.5 of the patch/code-editor spec), this
  path produces structural edits only. Nothing it does can make the file fail to
  project.
- **Creates no nodes.** It wires existing nodes and sets inline literals (§6).
  Building new DSP is what the create bubble is for.
- **Does not touch the existing bubble paths.** The create and edit prompts,
  their context, and their output types stay byte-identical. The isolation is
  structural, not conventional — see §3.

## 2. Trigger

`Cmd+Shift+K` with exactly one node selected, in any patcher view.

Deliberately a distinct binding rather than intent-detection on the prompt text:
"connect it" versus "make it brighter" is not a distinction worth resolving
probabilistically when a modifier key is unambiguous.

With no selection, or more than one node selected, the key is not consumed.

## 3. Target variant

```rust
AgenticBubbleTarget::ConnectNode {
    instance_node_id: String,
    subject: ConnectSubject,
}

enum ConnectSubject {
    Macro { name: String, params: Vec<String>, source: String },
    Operator { op: String },
}
```

The variant selects the agent's prompt template *and* its permitted output type.
A `ConnectNode` bubble may only return `AgenticBubbleOutput::Connections`;
`CreateMacro` and `EditMacro` bubbles may not return it. No prompt text is shared
between them, so neither can bleed context into the other.

Everything else about the bubble — grow-in, pending pulse, answer resize, escape
shrink-out, the `↳ name` header badge — is inherited unchanged.

## 4. Scope

The context is the **current view level only**, resolved through the existing
seam:

```rust
let view_key = active_patcher_view_key(&interaction);   // "root" | "macro:{name}"
let patch = active_patcher_patch(&root_patch, &interaction);
let patch = patch_with_interaction_state(patch, &interaction, &view_key);
```

Root and macro levels are the *same code path*, differing only in which patch
comes back. Both are closed scopes — a defmacro cannot reach outside itself, and
nothing exists above root — so the current level is not a lossy reduction for
context budget. It is complete and correct.

`patch_with_interaction_state` matters: without it the agent cannot see nodes the
user typed but has not committed.

**Size.** A macro scope is inherently small. Root is unbounded, so pruning is a
root-only concern. §5.2 emits one line per node rather than node internals, which
should keep even large patches tractable — to be measured against the largest
real patch before assuming (§10).

## 5. Context payload

### 5.1 Inlets are argument indices

This is the model fact everything else follows from. A node's inlets *are* its
`PatchNode.args`, addressed by argument index. Whether an argument draws a port
is derived (`patch_input_indices`):

```
port drawn at arg i  ⟺  args[i] ∈ { SymbolRef, ConnectedExpr }
                      ∧ inline_inputs[i].is_none()
```

An `ArgValue::Literal` draws **no port** — the value renders inside the node.
That *is* the inline form, and it is why inlining costs nothing visually: there
is no extra node and no cable, just text in the node that was already there.

Consequence for addressing: **argument indices are stable, drawn port positions
are not.** Setting a literal on arg 2 removes that port and shifts the remaining
ports leftward on screen. The plan therefore addresses argument indices, never
"the third port".

### 5.2 The subject

- `ConnectSubject::Macro` — the full `defmacro` source. Parameter *names* are
  what make "trigger input → trig inlet" inferable, and the body disambiguates
  when names are terse. Already carried on the bubble today:
  `AgenticBubbleTarget::EditMacro` holds both `params` and `source`.
- `ConnectSubject::Operator` — `dgenlisp_operator_documentation(op)`, the same
  `OperatorPortDocumentation` that feeds the port tooltips.

### 5.3 The surrounding patch

One line per node at the current level, plus one line per argument:

```
<id>  <label>  kind=<NodeKind>
  in  0  <name>  free
  in  1  <name>  cabled from <id>:<outlet>
  in  2  <name>  literal "0.5"
  out 0  <name>
```

Inlet names come from macro params or operator docs; outlet names from
`PatchNode.outputs`. `NodeKind::In` nodes are the level's signal sources at both
levels — at root the patch's inputs, inside a macro its parameters — so one
builder covers both.

### 5.4 Occupancy

Per argument index `i` of node `n`:

- **free** — a port is drawn at `i` per §5.1, and no `PatchConnection` with
  `presentation == Cable` targets `(n, i)`.
- **cabled** — a `Cable` connection targets `(n, i)`.
- **literal** — `args[i]` is `ArgValue::Literal`; no port is drawn.
- **inline param** — `inline_inputs[i].is_some()`; no port is drawn. Distinct
  from a literal: this is a named param reference rendered inline, backed by a
  param node that `hidden_inline_node_ids` hides.

Only **free** slots are valid plan targets (§7).

This section is load-bearing. "Connect it *if applicable*" is most of the
feature's value, and without occupancy the agent proposes values for slots that
already have them and cables for inlets already wired.

## 6. Plan schema

A flat, ordered list of tagged ops:

```json
{"ops": [
  {"op": "connect", "from_node": "trig-in", "from_outlet": 0,
                    "to_node": "created-3", "to_arg": 0,
                    "why": "patch trigger to macro trig"},
  {"op": "inline",  "value": "0.8",
                    "to_node": "created-3", "to_arg": 2,
                    "why": "decay needs a value to sound"}
]}
```

`inline` sets `args[to_arg]` to a literal. No node is created and no cable is
drawn — the value simply appears in the node, and its port goes away. This keeps
auto-generated wiring from adding clutter, which is the point: cables should mean
"signal flows here", not "a number lives here".

Inlining is not a dead end. A literal argument can be re-opened into a port later
by clearing it, and a modulatable param materialises a real input when something
is wired to it — so choosing inline now does not foreclose modulating it later.

**Names in, indices out.** The context gives the agent named ports so it can
reason about intent; the plan addresses arguments by index so the host resolves
them without fuzzy matching.

**Ids, never labels.** Two `+` nodes render identically. `PatchNode.id` is the
identity; `label` is decoration in the context and is not accepted in a plan.

`why` is short free text, surfaced in the status line so the user can see what
was done and why without diffing the canvas.

## 7. Validation

Every op is checked before anything is applied:

- `from_node` / `to_node` exist at the current level
- `from_outlet < outputs.len()`, `to_arg < args.len()`
- the target argument is **free** per §5.4 — an op targeting a cabled slot is
  rejected rather than silently replacing the cable, and one targeting a literal
  or inline-param slot is rejected rather than overwriting a value the user set
- no self-connection
- no two ops target the same argument
- for `inline`, `value` round-trips through `parse_editor_node_text` (§8)

Cycles are **not** rejected. Feedback is a first-class construct here
(`ConnectionKind::Feedback`), so a cycle is a legal thing to propose.

## 8. Apply

- **connect** — `allocate_created_connection(state, view_key, from, to)`, the
  same primitive a cable drag produces.
- **inline** — `parse_editor_node_text` yields `(op, Vec<String>)` for the target
  node; replace element `to_arg`, rejoin as `"{op} {args…}"`, and store it as the
  node's `PatcherNodeEdit.text` under `node_edit_key(view_key, node_id)` — the
  same edit a double-click and retype produces. Re-parse to confirm the op and
  arity are unchanged before accepting.

Both are edits the UI already makes, so the result is projectable by
construction and needs no acceptance check of its own.

The whole plan lands in **one** `set_patcher_interaction_state` write, so the
existing history hook records it as a single gesture: one Cmd+Z undoes the
entire wiring.

Applied immediately rather than previewed. The point is speed, Cmd+Z is one key,
and a confirm step on something usually obviously right gets tiresome fast.
Ghost-cable preview remains possible later on top of the drag-preview rendering
without changing anything above.

## 9. Partial application

An invalid op does not sink the plan. Valid ops apply; skipped ops are reported
in the status line with their reason. Silently dropping them would read as "it
wired everything" when it did not.

If *no* op survives validation, the bubble goes to `Error` with the reasons,
which makes Cmd+R retry work exactly as it does today.

## 10. Open questions

1. **Root context size.** Measure the §5.3 payload against the largest real
   patch before assuming one-line-per-argument is enough pruning. Emitting args
   only for nodes with free slots is the obvious first cut.
2. **Literal formats.** `inline` values are constrained by what
   `format_patch_literal` / `is_numeric_literal` accept. Worth pinning down
   whether anything beyond numbers is permitted before the agent is told it can
   emit them.
3. **Library macros.** A macro instance whose `defmacro` lives outside this file
   needs its source resolved through the library before §5.2 can supply it.
4. **Multi-select.** Spec'd as single-selection. "Connect these three" is a
   plausible extension the schema already permits.
