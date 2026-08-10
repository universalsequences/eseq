# Patch Editor vs Code Editor Split

Status: rev 3 — Phases 1 + 2 + 3 BUILT (commits 7df57e8d, 64814000, 72624fbc,
d9947253, 660340d8 on `patch-code-editor-split`); rev 3 adds Phase 5 (§4.1b
graph payload as load SOT), remaining: Phase 1b, 4, 5 + §8 deferred tail
Scope: editor selection + kill write-back. Bottom-panel layout redesign is explicitly OUT of scope (separate effort).

## 1. Motivation

Two authoring populations have diverged:

- **Hand-made patches** — small, deliberately organized, fun to build visually. The
  patch editor is the right surface, but write-back bugs keep breaking patches right
  as they get good, and there is no undo.
- **Agent-authored instruments/effects** — huge, and their dgenlisp source is the
  *good* representation (commented, structured). Projected into the patcher they are
  impenetrable node soup. A text buffer is the right surface.

Today everything opens in the patcher unconditionally
(`enter-edit-instrument`, `crates/sequencer/src/ui/host_commands/instrument_authoring.rs:615`),
and the patcher round-trips edits through an ~8k-line surgical AST rewriter
(`crates/eseqlisp/src/widget_render/patcher/writeback.rs`) that:

- discards all comments (tokenizer drops them: `crates/eseqlisp/src/lang/parser.rs:399`),
- flattens all formatting (`SourceDocument::emit` joins forms with `\n`, every list on one line),
- reorders forms (`forms_in_emit_order`, `generated_def_insertion_index`),
- refuses edits in 14 distinct `WriteBackError` cases (code islands, rename collisions,
  history edits, staleness guards, …).

Since write-back destroys hand-written formatting anyway, its only remaining value —
"preserve the user's code" — is already void. Killing it removes the whole class of
"patch broke at the worst moment" bugs.

## 2. Design overview

Two editing surfaces, one deterministic decision:

| | Patch editor | Code editor |
|---|---|---|
| Who | patch-authored content + all *new* creations | everything else (agent-authored, hand-coded) |
| Source of truth | in-memory patch model; disk = **serialized graph payload** in the sidecar (authoritative, §4.1b) + generated `dsp.lisp` (compiler/human artifact) | `dsp.lisp` text, verbatim |
| Code direction | one-way: patch → code (full regeneration, deterministic) | n/a (code *is* the artifact) |
| Compile trigger | automatic on every semantic edit (unchanged) | explicit "evaluate" (shortcut + button) |
| Write-back (code → patch) | **deleted** | n/a |

The existing preview/compile/hot-swap pipeline is shared unchanged by both surfaces:
background Draft compile (`compile_and_load_instrument_with_origin`,
`DGenSourceOrigin::Draft`) → polled in `event_loop.rs:1609` →
`apply_compiled_instrument_engine` / `apply_compiled_effect_edit_session` →
`session.last_valid_source`. Only the *source text producer* differs.

## 3. The authorship discriminant

### 3.1 Why "sidecar exists" is not enough today

The intended rule was: *has `dsp.layout.json` → patch editor; no layout → code editor.*
But `sidecar::apply_or_materialize` (`crates/eseqlisp/src/widget_render/patcher/sidecar.rs:77`)
**auto-writes a sidecar the first time anything is opened in the patcher** (that is why
`effects/spectral-cumsum-soothe/dsp.layout.json` exists, untracked). Measured on disk:
65 sidecars for 138 `dsp.lisp` files, all `version: 1`, and many belong to clearly
agent-authored effects (spectral-*, emulations/*). Sidecar presence currently means
"was ever opened," not "was patch-authored."

### 3.2 The rule

An instrument/effect is **patch-authored** iff BOTH:

1. Its layout sidecar exists with `version >= 2` and `"authored": true`
   (new field; bump `SIDECAR_VERSION`, `sidecar.rs:19`), AND
2. `parse_patch_source_with_library` succeeds with **zero code islands**
   (`patch.diagnostics` empty).

Editor selection:

- **Edit existing, patch-authored** → patch editor.
- **Edit existing, anything else** → code editor. All 65 existing v1 sidecars fall
  here by construction — per direction, we do not migrate old write-back-era patches;
  every existing item opens as code. (They all still work as instruments; nothing is
  lost except graph editing, recoverable via §3.3.)
- **Create new instrument/effect** → patch editor (patching stays the default happy
  path). On first save it writes an `authored: true` v2 sidecar.

Condition 2 makes the system self-healing: if a patch-authored file is ever edited by
hand or by an agent into something the projector can't fully represent, it silently
demotes to the code editor instead of showing a broken half-patch. It never blocks
opening — worst case you always get a working text editor.

### 3.3 Manual promotion ("Open as patch")

An explicit action (editor-header button or browser context action) on a
code-authored item: run the parse; if zero code islands, stamp the
`authored: true` sidecar (materializing default layout) and reopen in the patcher.
If code islands exist, refuse with the diagnostic list. This is the recovery path for
the user's genuinely hand-patched pre-v2 items, opt-in per item.

**Rev 3:** promotion writes a fresh v3 graph payload from the projection it just
ran, and reuses only *positions* from any pre-existing sidecar — a stale `graph`
must be discarded, else promoting after an eject-and-edit resurrects the
pre-eject model (§4.1b).

### 3.4 Demotion / "Eject to code"

Explicit action in the patch editor: set `authored: false` (keep the sidecar for
possible re-promotion), reopen in the code editor. The generated `dsp.lisp` is already
on disk and canonical, so ejecting is free and lossless. One-way in spirit: after
hand-editing, re-promotion only succeeds if the code still parses cleanly.

**Rev 3:** eject is the *only* sanctioned way a patch-authored item's source gets
hand-edited, and that is what lets §4.1b skip drift detection entirely. While
`authored: false` the graph payload is stale by design and never consulted;
re-promotion rebuilds it from the edited source.

### 3.5 Stop auto-materializing sidecars

`apply_or_materialize` must stop writing to disk on open (`save_patch_layout` call at
`sidecar.rs:222` path). Sidecars are written only on explicit save (and by the
promotion action). This stops the signal pollution at the source. Existing stray
sidecars are harmless (they're v1 → code editor) and can be deleted at leisure.

## 4. Patch editor: kill write-back, generate code

### 4.1 New model

`dsp.lisp` for a patch-authored item is a **build artifact of the patch model**,
regenerated *in full* on every semantic edit. Parsing machinery
(`parse_patch_source`, `Projector`) is kept unchanged; only the *reverse
surgical* machinery dies.

Rev 1–2 made `dsp.lisp` + layout sidecar the durable SOT and loaded the model by
re-parsing the file. Rev 3 supersedes that: see §4.1b. The durable SOT is the
serialized graph payload; `dsp.lisp` remains the compiler input and the artifact
humans and agents read, but is no longer parsed to reconstruct the editor model.

### 4.1b The graph payload is the load SOT (rev 3)

**Rule: the serialized patch model is authoritative on load. Every path that
produces a model by *projecting from source* writes a fresh payload in the same
step.**

Rev 1–2 made only one direction deterministic — model → source. The reverse
(source → model, `Projector::project`) stayed heuristic, because the generated
lisp is a lossy encoding of the model: distinct models can print to identical
text. The consequences were not theoretical, and were being paid for in the
generator:

- **Inline literals vs constant nodes.** `(mymacro a 0.3 b)` is textually
  identical whether `0.3` was typed inline in the node box or fed by a separate
  constant node. The projector resolves it by rule
  (`flush_pending_constant_args`, `project.rs:916`): an inline numeric literal
  becomes a wired `NodeKind::Constant` as soon as *any later argument in the
  same call is connected*. To keep round-trips stable, the generator preemptively
  conceded — `literal_needs_hoisting` (`generate.rs:741`) hoists such a literal
  into a standalone `(def …)`. Net effect: saving `mymacro ? 0.3 ? 0.9` and
  reopening it yields a number node cabled into the inlet. The round-trip
  property held; the model was degraded to make it hold.
- **Nodes absent from the source.** §4.2b's dead+incomplete nodes are already
  persisted through the sidecar and re-added on load
  (`overlay_scope_visible_layout`, `sidecar.rs:654-669`) — so the sidecar was
  *already* a partial graph store, carrying just enough to be load-bearing and
  not enough to be authoritative.
- **Input presentation.** Inline-vs-cable is sidecar-recorded but re-validated on
  load and silently downgraded to `Cable` when it cannot be resolved
  (`refresh_patch_inline_inputs`, `model.rs:316-354`), which no constant node
  ever can (`param: None`).

Under rev 3 the sidecar carries a complete `Patch` serialization (§4.1c) and the
load path deserializes it directly. Distinctions the lisp cannot express no
longer need to survive a parse, so the generator stops distorting its output to
carry them.

**Drift is handled by precedence, not detection.** There is deliberately no hash,
no divergence check, and no reconciliation. A patch-authored item's `dsp.lisp` is
not hand-edited: to edit code you eject (§3.4), which sets `authored: false` and
routes the item to the code editor, where the payload is simply not consulted.
Re-promotion (§3.3) re-projects the *edited* source and writes a fresh payload —
hand edits therefore land in the graph by the ordinary promotion path.

The projecting paths that must write a payload are exactly three:

1. **Promotion** (`promote_source_to_patch`, `mod.rs:104`). Reuses pre-existing
   sidecar *positions* only — it MUST discard any stale `graph`, or promoting
   after an eject-and-edit resurrects the pre-eject model and discards the code
   edits it was supposed to absorb.
2. **The agentic macro-edit flow** (`rematerialize_edited_macro_layout`,
   `mod.rs:489`), which projects source the Cmd-K bubble has just rewritten.
   Without a payload write, a stale payload would silently revert agent edits on
   the next open.
3. **First open of a pre-rev-3 sidecar** — project once, materialize the payload
   on next save. Same self-healing shape as the v1 → v2 step (§3.2).

### 4.1c Sidecar v3 format

`ScopeLayout` (`sidecar.rs:56`) gains a `graph` payload holding the full `Patch`:
nodes (`id`, `kind`, `op`, `args`, `label`, `param`, `width`), connections
(including `presentation`), macro scopes, and `host_modulators`. Positions and
cable layout stay where they are. Version bumps to 3; v1 and v2 continue to load
(v1 still counts as unauthored per §3.2), a v2 sidecar simply has no `graph` and
takes the project-once migration path above.

Once the payload is authoritative, three pieces of scaffolding lose their reason
to exist and are removed: `literal_needs_hoisting` and its call site
(`generate.rs:700-708`, `:741`) — the generator emits inline literals as inline
text; the omitted-node re-add in `overlay_scope_visible_layout`
(`sidecar.rs:654-669`); and the inline-presentation downgrade in
`refresh_patch_inline_inputs` (`model.rs:326-342`).

### 4.1a No source mapping — the hard rule

The old system's fragility came from tracking *where each node lives in the code*
(`NodeSource { owner, expr: SourceExprId, call_shape }`, `model.rs:99-165`) and
reconciling edits against those positions. The new system must not reason about
source positions **anywhere**:

- **Editing**: every gesture mutates the in-memory `Patch` model directly. The model
  is the only thing edits touch; source text is write-only output.
- **Identity**: each node carries a stable id assigned at creation and kept for life.
  The generator emits that id as the node's binding name (e.g. `(def osc-3 …)`), and
  the layout sidecar keys positions by the same id. Reopening a patch recovers
  identity purely from binding names — never from expression paths, ordering, or
  structural matching. `emitted_layout_json_with_node_map`-style identity overlay
  machinery (sidecar.rs:179) is no longer needed.
- **Loading** is the parser's only remaining job: canonical file → model, once, at
  session open. `NodeSource`/`SourceExprId` fields become dead for patch-authored
  content (they may linger in the struct for the demotion check in §3.2, but nothing
  in the editing or generation path may read them). Any future feature that "needs to
  know where a node is in the file" is a design smell — the answer is always
  "regenerate the file."
- `place_unmatched_nodes` (sidecar.rs:340) survives only as the fallback for
  promotion (§3.3) and for ids that appear in code but not in the sidecar.

### 4.2 Deterministic generator

New emitter (seed: `emit.rs::emit_patch_debug_lisp`, currently debug-only) replacing
`patcher_writeback_payload`'s surgical path. Requirements:

- **Deterministic**: identical patch model → byte-identical output. Stable topological
  node order with a stable tiebreak (node id), stable binding names.
- **Readable**: pretty-printed, one `def`/form per node group, indented multi-line
  output — this file is what git diffs and what agents read after an eject. A header
  comment marks provenance: `;; generated by the patch editor — edit via the patcher
  or eject to code`.
- **Position-blind**: the generator's input is the patch model alone — node ids,
  ops, attributes, connections. It never consults prior source text, prior form
  order, or any `NodeSource` data (§4.1a). Same model → same file, regardless of
  the patch's edit history.
- **Complete**: covers everything the patcher can express — nodes/attributes,
  connections, input presentations, local `defmacro`s, staged defmacro-library
  imports (`compile-source` materialization stays as today, mod.rs ~2409).
- Round-trip invariant (test): `parse(generate(patch)) == patch` (modulo layout),
  and `generate(parse(generate(patch))) == generate(patch)`. **Rev 3:** this is no
  longer the correctness contract for *editing* — the editing contract is
  `deserialize(serialize(patch)) == patch` (§4.1b). The parse round-trip is
  retained as the fidelity contract for the projecting paths (import, promotion,
  agentic edits), and is expected to lose distinctions the lisp cannot express
  (inline literal vs constant node) rather than force the generator to encode
  them.

### 4.2a Created-node identity never persists

Interaction ids minted by `allocate_created_node` (`created-N`) are session
identities, not names. The generator maps them to deterministic op-derived
bindings (`* 2` → `mul`, `cos` → `cos-2` — operator names are reserved so a
binding never shadows an op), reporting the mapping in `renamed_node_ids` so
the layout sidecar follows. Persisting `created-N` as a binding is forbidden:
a reload parses it back as a node id, and the next session's counter collides
with it — the existing node's cables visually splice into the newly created
node and regeneration rewires a→b into a→c→b. As a second guard, allocation
(`allocate_created_node_avoiding`) skips any id already present in the model,
which also heals sources written before this rule existed.

### 4.2b Dead code is not emitted

Liveness = "reaches some `out` node" (reverse reachability over all
connections, feedback included). Value nodes that are dead AND whose call is
incomplete — empty/unknown op, a missing-input gap, or an unfilled macro-
instance arity — are omitted from the generated source; they persist only in
the interaction state and the layout payload (matching the pre-regeneration
system, where uncommitted incomplete nodes never entered the source). Dead
nodes with complete calls ARE emitted as unused defs: omitting them would lose
them on save + reload. A disconnected created macro instance therefore never
produces a call — the `defmacro` definition still persists — while a LIVE
node with a missing input keeps the missing-input sentinel (and macro calls
emit their full arity), so genuinely broken patches still surface diagnostics.

"Macro-instance arity" here covers the dgenlisp *preamble* defmacros too
(`svf`, `adsr`, `polyblep`, … — the standard-library macros the backend
attaches to every compiled source, identified by the `preamble` category in the
bundled operator manifest that already drives the node's inlet count): they
expand with a fixed parameter list, so a dead unwired instance is omitted like
any other macro instance rather than emitted as an arity-error `(svf)`.

The "complete dead nodes survive" rule covers user-authored nodes only:
projector-synthesized helper nodes are never persisted once orphaned. The one
such helper today is the hidden `(mod p)` accessor the projector nests behind
the `p~` sugar — the user never authored it and never sees it. When its only
consumer is deleted (`(* a gain~)` goes away), the accessor is garbage-
collected with it, both in `patch_with_interaction_state` (so it never pops
into view as a bare `gain -> mod` node) and in the generator's omission set
(so it never lands in the source as `(def mod0 (mod gain))`). The
discriminant is the source owner: `NestedExpr` = synthesized helper,
`BindingValue` = a real `(def m (mod gain))` the user wrote, which round-trips
unchanged even when unused.

### 4.3 What gets deleted

- All surgical edit application in `writeback.rs`: `SourceDocument` mutation paths
  (`replace_expr`, `apply_node_deletions`, `apply_cable_writeback`,
  `apply_history_writeback`, staleness guards `node_edit_is_stale` etc.).
- The `WriteBackError` refusal surface — edits can no longer fail structurally.
  (`EditedCodeIsland` becomes moot: authored patches contain no code islands by
  invariant; the code-island node type remains only as a projector diagnostic that
  triggers demotion, §3.2.)
- `PatchEditState` as a *diff-against-source*: the interaction state can now mutate
  the patch model directly, since there is no source document to reconcile against.
  (Implementation may keep the struct; the semantic is "the model," not "a pending
  diff.")

Event flow after the change: semantic edit → mutate model → `generate()` →
payload `{status: :valid, source, compile-source, layout}` → same
`preview-instrument-patch` / `preview-effect-patch` handlers → compile/hot-swap.
Layout-only edits still short-circuit (`status: :layout`, no recompile). Save writes
generated source + v2 sidecar (existing save paths at `instrument_authoring.rs:804`,
`:1969` unchanged in shape).

### 4.4 Patch editor undo (enabled by this change)

Write-back's death makes undo tractable: an undo step is a snapshot of
(patch model, layout) per committed gesture, restored by regenerate + recompile.
No source reconciliation, no staleness. Recommended: a bounded snapshot stack inside
`PatcherInteractionState`, Cmd-Z routed to the patcher when it has focus (today
`sequencer_history_shortcut`, `crates/sequencer/src/ui/input.rs:85`, already bails
for patcher text edits; extend the bail to feed patcher undo instead of nothing).
App-global `EditPatch` integration is NOT required — patcher undo is local to the
edit session, like text-buffer undo. Phase 3; specced here so Phase 2 keeps
model snapshots cheap (the model must be cheaply cloneable).

### 4.5 Agentic edits (Cmd-K bubble)

Agent returns code. For a patch-authored item: accept only if it parses with zero
code islands (then it *becomes* the model and is regenerated canonically — the
agent's formatting/comments are not preserved, consistent with §4.1); otherwise
offer eject-to-code with the agent's text intact.

## 5. Code editor surface

### 5.1 Buffer + layout

Reuse the editor stack — this mostly already exists:

- The patcher's Tab-split source view is a real `BufferMode::DGenLisp` buffer,
  forced read-only (`upsert_patcher_emitted_source_buffer`,
  `crates/eseqlisp/src/editor/mod.rs:4837`, `read_only = true` at :4535).
  The code editor is a writable sibling: open `dsp.lisp` contents into a
  `*instrument-code:<name>*` buffer, `BufferMode::DGenLisp`, writable.
- New layout mode `:instrument-code` mirroring `:instrument-patcher`
  (`seq-instrument-patcher-layout-spec`, `crates/sequencer/ui/main.lisp:600`):
  code buffer in the main region, same bottom bar (unchanged per scope note),
  same `sbrowser-editor-header` save pane.
- Session state: add a surface discriminant to `InstrumentEditSession` /
  `EffectEditSession` (`crates/sequencer/src/ui/edit_sessions.rs:53` / `:316`),
  e.g. `EditorSurface::{Patch, Code}`, chosen by §3.2 at `enter-edit-*` time.
- In code mode the patcher Tab source-split is n/a; in patch mode the read-only
  emitted-source preview stays as-is (it now shows the canonical generated code).

### 5.2 Explicit evaluate (compile + hot-swap)

Typing must not auto-compile — mid-edit text is usually invalid. Instead:

- New host command `evaluate-editor-source`: read the code buffer text, run it
  through the *existing* preview pipeline — materialize defmacro imports
  (`materialize_defmacro_imports`, `effect_compile.rs:280`), background Draft
  compile, same `PendingInstrumentPreview` polling, same
  `apply_compiled_*` hot-swap, same `SEQ.editor-error` / "Preview compiling…"
  status row in `sbrowser-editor-header`. Compile errors land in the same
  status row (with the compiler diagnostic).
- Keybinding via the existing declarative layer: `define-mode` a
  `seq-dgen-code-mode` and `mode-bind-key` **`C-c C-c`** (Emacs eval-buffer
  convention; the bind-key infra is `crates/eseqlisp/src/editor/natives.rs:70/:311`).
- Plus an **Eval** button in `sbrowser-editor-header` (browser.lisp:1191),
  shown only when the session surface is Code.

### 5.3 Save semantics (comments must survive)

Save writes the **buffer text verbatim** to `dsp.lisp` — never `last_valid_source`,
never `compile-source` — so comments and formatting are preserved. Guard: if the
buffer has not been successfully evaluated since its last change, the save button
warns ("unevaluated changes — Eval first or Save anyway"); we still allow save,
since a text file that fails to compile is recoverable in a way a broken patch never
was. No sidecar is written for code-authored items.

### 5.4 Autocomplete: one data source

The text editor's DGenLisp completion currently uses hand-maintained tables
(`DGENLISP_SPECIALS` / `DGENLISP_BUILTINS`, `crates/eseqlisp/src/mode.rs:206/:245`)
— disjoint from the patcher's real source,
`crates/sequencer/tools/dgenlisp-operators.json`, already embedded and cached in
eseqlisp (`dgenlisp_operator_names` / `dgenlisp_operator_documentation` etc.,
`patcher/project.rs:1284-1494`). Fix: `BufferMode::DGenLisp` completion draws from
those same accessors (operators + constants + attributes), keeping the hand tables
only for true special forms. Doc-panel rendering in the text editor (the patcher's
`push_autocomplete_documentation_panel` equivalent) is a nice-to-have, phase 4.

## 6. Out of scope / deferred

- Bottom-panel consolidation (reduced mixer strip, merged save/mixer pane) — separate spec.
- Migrating/repairing pre-v2 patches automatically (manual promotion covers it).
- `ui.lisp` editing in the code editor (second tab later; panels are regenerated on
  save today for patcher items and untouched for code items).
- Text-editor doc panel + fuzzy completion.
- "New instrument via code" entry point (today: create in patcher, eject; or author
  on disk and edit). Add later if the friction is real.

## 7. Open questions (resolved 2026-08-07)

1. Evaluate shortcut: **`C-c C-c`** (decided; via `mode-bind-key`, no hardcoded Rust layer).
2. ~~Does the editor buffer stack have text undo already?~~ Resolved: yes —
   `TextUndoSnapshot` / `undo_stack` / `undo_text` (`crates/eseqlisp/src/editor/mod.rs:496,658,5723`),
   with typing-group coalescing. The code editor gets undo for free.
3. Should eject delete the sidecar's layout data or keep it for re-promotion?
   (Spec says keep, `authored: false`.)
4. Where does "Open as patch" live for code items — editor header, or browser row action?
   Still open; Phase 2 concern.
5. Phase 1 escape hatch for existing patch-authored items: **decided NO** — during the
   transition, all existing items are code-only; graph editing of old patches returns
   with Phase 2 promotion (§3.3). Phase 1 stays simpler.

## 8. Phasing

- **Phase 1 — the split. BUILT 2026-08-07** (branch `patch-code-editor-split`).
  `EditorSurface` discriminant (§3.2 rule, reading v1 sidecars as "not
  authored"), writable `*instrument-code:*`/`*effect-code:*` buffers (the
  existing patcher layout fn is buffer-generic, so no new layout mode was
  needed), `evaluate-editor-source` + `C-c C-c` + Eval button, verbatim save,
  stop sidecar auto-materialization (§3.5) — with one deliberate exception:
  `apply_or_materialize_excluding_macro_scopes` still writes, because it runs
  inside the agentic macro-edit flow which has already rewritten dsp.lisp, and
  skipping it would let a stale macro scope poison later loads. Sidecar format
  is now v2 + `authored: true` on every save; v1 still loads. Patch editor
  internals otherwise untouched.
- **Phase 1b — autocomplete unification** (§5.4). Independent, small. Deferred by
  user choice; not started.
- **Phase 2 — kill write-back. BUILT 2026-08-07** (commit 64814000 + fixes 72624fbc).
  Deterministic generator (`patcher/generate.rs`) with round-trip + real-compiler
  tests, payload switch in `patcher_writeback_payload`, promotion
  (`promote-editor-to-patch` + "Open as patch") and eject (`eject-editor-to-code`,
  layout kept for re-promotion), agentic-edit rule (§4.5). Post-build fixes added
  §4.2a (interaction `created-N` ids never persist as bindings; allocation skips
  taken ids, healing poisoned files) and §4.2b (dead-code emission policy:
  liveness = reachability from `out`; dead+incomplete omitted but layout preserved,
  dead+complete emitted as unused defs, defmacros always persist). Two model gaps
  closed along the way: `Patch.host_modulators` (hidden `@modulator` defs now
  round-trip) and top-level `(param …)` emission (dgen `(mod name)` resolution).
- **Phase 3 — patch editor undo** (§4.4). BUILT: bounded `PatchEditState`
  snapshot stacks per widget (`PatcherHistory`, state.rs), recorded centrally in
  `set_patcher_interaction_state` with gesture coalescing (open drag/text edit =
  one step); Cmd+Z/Cmd+Shift+Z handled in the patcher `key_event`, with
  `sequencer_history_shortcut` yielding to any focused patcher. History drops on
  `reset_patcher_widget_state` (one base-source epoch). Node-selection
  copy/paste (Cmd+C/Cmd+V, process-local clipboard with internal-wire remap)
  landed alongside.
- **Phase 4 — polish.** Text-editor doc panel, "new via code" if wanted. Not started.
- **Phase 5 — graph payload as load SOT** (§4.1b/§4.1c, rev 3). Sidecar v3 with a
  full `Patch` serialization; load path deserializes instead of projecting;
  payload writes on the three projecting paths; delete `literal_needs_hoisting`,
  the `overlay_scope_visible_layout` omitted-node re-add, and the
  `refresh_patch_inline_inputs` downgrade. Motivating bug: inline macro args
  (`mymacro ? 0.3 ? 0.9`) reopening as cabled number nodes.

### Deferred tail (known, deliberate)

- **Full `writeback.rs` deletion**: the surgical `SourceDocument` machinery still
  powers the macro-library flows (save-to-library / fork / staged-edit flush) and
  the agentic candidate splice. Rebuilding those on the generator unlocks deleting
  the rest (~7k lines and its ~300 tests migrate or die with it).
- **Builtin-call attributes** (e.g. `@max-delay` on an arbitrary call): not
  represented in the patch model, so promoting source that uses them and then
  editing regenerates WITHOUT them — silent loss. Either reject at promotion
  (§3.3) or add attribute storage to `PatchNode`.
