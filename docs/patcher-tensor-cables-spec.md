# Patcher: tensor cables, cable shapes, and node-located compile errors

Status: rev 1, 2026-10-13. Epic: `eseq-cyk9`.
Compiler side: dgen-audio PR #27 (`feat/binding-shapes`).

## 1. Problem

Tensors flow through patch cables exactly like Max's `mc` multichannel
signals: `iota 8 → phasor → * twopi → cos → sum` carries 8 lanes on every cable
up to `sum`, and one after it. The patch editor draws every cable the same, so
nothing shows which cables carry tensors or what shape, and shape mistakes
(`[8 1]` against `[8]` broadcasting to `[8 8]`) are invisible until the
compiler rejects the patch. A compile error, when there is one, is a line of
text; it does not point at the node.

## 2. Principle: the compiler is the only shape authority

The editor never re-derives shapes. The patcher already emits one top-level
form per node, named after the node (`(def phasor-2 …)`, `(make-history h …)`),
and keeps the node ↔ binding-name mapping (`GeneratedPatchSource::
renamed_node_ids`; an unrenamed node's binding is its id). The compiler reports
each top-level binding's shape; the editor maps it back to the node. A cable's
shape is its source node's output shape.

## 3. Compiler contract (dgen PR #27)

- Manifest field `bindingShapes`: binding → `{kind, shape, elements?}`, kind ∈
  `signal`, `constant`, `tensor`, `signal-tensor`, `tensor-history`, `tuple`,
  `none`. Macro-local defs are not reported; a macro call site is.
- `failedForm` when evaluation throws: `{index, op, name, message}`. `name` is
  the def's binding / the history a write targets — the node to mark. Errors
  inside a macro body are attributed to the calling top-level form.
- `<dylib-name>.shapes.json` in the compile's output dir, written whether or
  not evaluation succeeds: `{bindingShapes, failedForm?}`. A failed compile
  writes no manifest, so this is how failures report.

## 4. Host plumbing (eseq)

- `DGenManifest` parses `bindingShapes` (absent → empty: older compilers).
- `compile_effective_dgen_source_to_dir` reads `<dir>/<dylib>.shapes.json` on
  both paths. Success: the manifest's copy is authoritative (and survives the
  dylib cache, which persists the manifest). Failure: the report is recorded in
  a process-global "last compile report" keyed by the effective source hash, so
  the preview path can fetch it alongside the error string without changing
  every compile caller's `Result<_, String>`.
- The preview completion (event loop) hands the patcher, keyed by the session
  path: the binding shapes, and on failure the failed form + message.
- The patcher resolves binding → node with the emission's map (the inverse of
  `renamed_node_ids`, falling back to id = binding), and stores per-node output
  shapes in derived state (not undo history; like the selected-asset mirror).
  A multi-output node emits a destructuring def (`(def (re im) (fft …))`), so
  the compiler reports each output binding, not the node: the emission's
  per-node output names reassemble them into a tuple whose elements are the
  outlets' shapes (unknown unless every output was reached). A failure names
  the first output, which maps to the node too.
  Stale-safe: shapes from an older emission are kept until the next result;
  nodes absent from the latest result have unknown shape.

## 5. Drawing

- A cable whose source node outputs a tensor (`signal-tensor`, `tensor`,
  `tensor-history`, any shape with > 1 element) draws as a multichannel cable:
  thicker (≈1.6×), its continuous stroke alternating two colours along its
  length like Max's mc cables: `:patcher-tensor-cable` (a blue) and
  `:patcher-tensor-cable-alt` (a warm contrast); all complete themes define
  both. Cables carry no text: the shape shows in the inspector when a cable is
  selected (§6). Scalar and unknown-shape cables draw as today. Feedback
  cables keep their feedback colour as the primary run but take the alternate
  and width when they carry tensors; a selected tensor cable takes the
  selection colour as its primary run.
- A node's diagnostic (authored or compile failure) is a chip attached to the
  node's right edge on its header row, not below it.
- A node whose top-level form failed gets the error border (the existing
  `node.diagnostic` path) and the compile message as its diagnostic tooltip;
  cleared by the next successful compile or an edit to that node.
- The toolbar's red error still shows; it is prefixed with the node (the
  compiler's `at (def b):` text).

## 6. Inspector

- Selecting a cable shows its shape: `Cable · signal-tensor [8 1] · 8 lanes`,
  from `<source node> outlet N` to `<dest node> inlet M`.
- Selecting a node adds `Output: [8]` (or `scalar`) under its title.
- Unknown (not compiled yet / compile failed before it): `shape unknown`.

## 7. Not in this rev

- Shapes inside macro views (a body's shapes depend on the call site; a later
  rev can show the shapes for the instance the view was opened from).

- A fast check-only compiler mode (skip C codegen) for lower latency.
