# Scripts are modules — retiring the Scripts tab

Rev 3 (2026-08-27). Rev 1 proposed a `use-package` attachment form, entry
hooks, and a browser Packages tab as one unit; review simplified it: the
attachment mechanism is plain `(import …)` typed in *scratch*, and the
Packages tab is deferred to v0.2 as sugar over that same mechanism.
Rev 3 replaces rev 2's "retarget the new-script draft flow" affordance
with the unified *packages* view (§5): the draft/save-script flow is
deleted, not retargeted — it never had real users (every historical
script was an agent-written file), and creation, browsing, editing, and
attaching become one dired-like text surface.
Extends `docs/module-system-spec.md` (rev 3, S0–S5 built).

## 1. Problem

The Scripts tab conflates three things the module system now separates:

1. **A code format**: headerless `.lisp` under `content/scripts/**` that
   re-defs a seven-name convention contract pinned to `eseq.vanilla`
   (`script-buffer-name`, `script-init-fn`, … — keyspace (1) in
   `content/ui/seq-script-picker.lisp`). This is the "host calls into a
   user file by convention" seam module-spec §6 was built to eliminate.
2. **A project-attachment record**: loading a script appends a literal
   `(load (seq-project-content-path path))` line to the project scratch
   (`seq-script-append-to-scratch`). Path-based records are relocation
   hazards (`project.rs:2791` already rewrites one legacy family) and
   freeze into user project files at v0.1.
3. **A browsing UI**: the browser scripts tab.

## 2. Decision

**A "script" is just a module on the load path, and attaching it to a
project is just an `(import …)` line in *scratch*.** No new form, no
manifest ceremony, no convention contract. The scratch buffer is the
project's init file.

This works today with zero new mechanism (verified 2026-08-25):

- The scratch replays on all three runtimes — UI
  (`ui/state_values/project_state.rs:232`), scheduler
  (`scheduler/worker.rs:190-200`, full re-eval per version bump), midi-fx
  (`app/effects.rs:232`).
- `(import NAME)` is top-level, load-once, idempotent
  (`lang/compiler.rs:2411`), so it is safe under the scheduler's
  full-scratch re-eval.
- Every runtime gets the same scoped load roots
  (`lisp_host/shared_state.rs:406`, `ui/editor_setup.rs:47`): the manifest-free
  local workspace (`~/.eseq.d/packages/local`, no prefix), installed package
  `src/` roots (namespace-scoped, user tier shadows factory tier), and the
  factory content root. `~/.eseq.d/packages/local/my/riffs.lisp` with a
  `(module my.riffs)` header is importable as `(import my.riffs)` with no
  other ceremony.
- A module registers its own UI (step tabs via `eseq.seq-step-tabs`,
  widgets, macros) from its own namespace at first evaluation. N scripts
  coexist because of namespacing — the reset/`script-init-fn` dance
  becomes unnecessary, not reimplemented.

What makes this viable UX rather than a power-user trapdoor is two
affordances, both v0.1:

1. **Import autocomplete** (already filed: eseq-mods.15). Completing
   `(import …)` against everything resolvable on the load path — user
   modules, installed packages, factory modules — is the discovery story.
   It becomes load-bearing here and should rise to P1.
2. **The *packages* view** (§5): one dired-like text surface for
   creating, browsing, editing, and attaching packages. It replaces the
   new-script draft session flow (`ui/host_commands/scripts.rs`,
   `ui/edit_sessions.rs:848`), which is deleted with the Scripts tab —
   not retargeted; it never had real users.

## 3. Naming note

Module-spec §8 reserves `eseq.*` and single-segment names for core. Local
scripts in the manifest-free `packages/local` workspace should default the
template header to a neutral personal prefix (template uses `my.<name>`; the
user can rename). Enforcement stays policy, not code.

## 4. Migration / retirement (v0.1)

- `content/scripts/**` (27 dev-era demos) gets curated as part of the
  v0.1 content pass. Keepers gain module headers and move under the
  factory root at module-shaped paths so they are importable; the rest
  are deleted. The factory root is already an unprefixed load root, so no
  loader work is needed.
- Existing projects' `(load "content/scripts/…")` scratch lines: known
  kept-demo paths get a rewrite to the module import in
  `migrate_factory_content_paths` (`project.rs:357`); `load` itself is not
  removed, only demoted from blessed mechanism.
- The Scripts tab, the script picker's load flow, and the seven
  `eseq.vanilla/script-*` contract defs are deleted once the keepers are
  migrated. The two Rust emitters of the contract template
  (`ui/edit_sessions.rs:848`, `ui/host_commands/scripts.rs:178`) are
  deleted with the draft flow (§5); the *packages* view's create verb is
  the replacement.

## 5. The *packages* view (v0.1, eseq-mods.16)

A text-only, dired-like mode that takes over the *sequencer* buffer
(the largest tile) as a direct line to the manifest-free personal workspace
at `~/.eseq.d/packages/local/`. Entered via a shortcut / `M-x packages`;
exiting (`q`, Esc) restores the previous tab. Ordinary personal modules need
no manifest; sibling package directories use manifests only when they need
distribution metadata, dependencies, an entry point, or verified external
assets. Four verbs share one surface:

- **Browse**: walk the modules tree dired-style. Each file row shows the
  filename plus the `(module …)` name read from its header, so the exact
  import spelling is always visible.
- **Edit**: `RET` on a file is plain find-file — opens it as a
  file-backed buffer and dismisses the view.
- **Create**: a name field at the top; `RET` on a name that resolves to
  no existing file creates it (find-file-on-missing semantics). Dotted
  names create intermediate directories per the load-root mapping
  (`my.euclid.sparse` → `my/euclid/sparse.lisp`), and the file is born
  with the correct `(module my.euclid.sparse)` header plus commented
  guidance (see template note below). A preview line under the field
  shows the resolved path and header before the file exists.
- **Attach**: a shortcut on a row inserts that module's
  `(import <name>)` line into *scratch* ("load into this project"). A
  second shortcut inserts it into `~/.eseq.d/init.lisp` instead
  ("attach to every session" — the home for §6.1 `override` /
  look-and-feel packages; a companion jump-to-init.lisp command makes
  the config file itself one keystroke away).

**Attach invariant** (unchanged from rev 2): attach only ever inserts
the textual `(import …)` line; the scratch/init replay does the actual
loading. Never a parallel mechanism. Consequences fall out for free:

- The listing's attached-state markers (✓ for *scratch*, a distinct
  mark for init.lisp) are *derived* by scanning those buffers' text for
  import lines, not stored anywhere. Attach on an already-attached row
  is a no-op / jump-to-line; v0.2 detach is "remove the line".
- Idempotent under the scheduler's full-scratch re-eval, like any
  import.

**Single focus model**: the name field and the listing share one focus —
typing filters the listing incrementally *and* stands ready to be a new
name when nothing matches (the dired/ido hybrid). Browse and create are
one gesture: type until you find it or until it doesn't exist, then RET.

**Template**: the created file opens with the module header, short
comments explaining both attachment destinations (*scratch* via the
view's attach shortcut; init.lisp for per-session config), a commented
example of registering UI from the module's own namespace, and an empty
`(export )`. No seven-name contract.

**Deleted with this**: the new-script draft session flow —
`script_draft_session`, `sbrowser-script-save-mode`, the
`new-script`/`save-new-script`/`cancel-new-script` host commands, and
the two Rust template emitters (`ui/edit_sessions.rs:848`,
`ui/host_commands/scripts.rs:178`).

## 5b. Deferred to v0.2: widening the view (eseq-mods.18)

Not a separate browser tab — the same surface widened: factory modules
and installed packages (with their manifests) join the listing, and
"detach" (remove the import line) joins the verbs. Rows and actions in
v0.1 should be designed so this widening is additive.
Also deferred with it: single-file synthesized-manifest packages,
`use-package`/entry evaluation, `on-attach`/`on-detach` hooks, git-install
UI (`src/package_install.rs` stays eval/CLI-level), and any "promoted
production-ready scripts" curation surface. Rev 1 of this spec (git
history) holds the worked design for that layer.

## 6. Out of scope entirely

- Def-unloading on detach / project switch (eseq-jo7.21's problem; the
  import lines in scratch are the attach list that fix will want).
- Version selection for package deps; remote registry.
