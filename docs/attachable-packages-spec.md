# Scripts are modules — retiring the Scripts tab

Rev 2 (2026-08-25). Rev 1 proposed a `use-package` attachment form, entry
hooks, and a browser Packages tab as one unit; review simplified it: the
attachment mechanism is plain `(import …)` typed in *scratch*, and the
Packages tab is deferred to v0.2 as sugar over that same mechanism.
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
  (`lisp_host/shared_state.rs:406`, `ui/editor_setup.rs:47`): user modules
  dir (`~/.eseq.d/modules`, no prefix), package `src/` roots
  (namespace-scoped, user tier shadows factory tier), factory content
  root. `~/.eseq.d/modules/my/riffs.lisp` with a `(module my.riffs)`
  header is importable as `(import my.riffs)` with no other ceremony.
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
2. **A "new script" affordance**: one command that creates
   `~/.eseq.d/modules/<name>.lisp` from a template (module header +
   commented example), opens it as a file-backed source tab, and leaves
   the user one `(import <name>)` away from attaching it. The existing
   new-script draft session flow (`ui/host_commands/scripts.rs`,
   `ui/edit_sessions.rs:848`) is the code to retarget; its template loses
   the seven-name contract and gains a module header.

## 3. Naming note

Module-spec §8 reserves `eseq.*` and single-segment names for core. Local
scripts in the user modules dir should default the template header to a
neutral personal prefix (template uses `my.<name>`; the user can rename).
Enforcement stays policy, not code.

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
  (`ui/edit_sessions.rs:848`, `ui/host_commands/scripts.rs:178`) migrate
  with the new-script template.

## 5. Deferred to v0.2: the Packages tab

A browser tab listing everything importable on the load path (user
modules, installed packages with their manifests, factory modules), where
"attach" inserts the `(import …)` line into scratch and "detach" removes
it — pure sugar over the same textual record, never a parallel mechanism.
Also deferred with it: single-file synthesized-manifest packages,
`use-package`/entry evaluation, `on-attach`/`on-detach` hooks, git-install
UI (`src/package_install.rs` stays eval/CLI-level), and any "promoted
production-ready scripts" curation surface. Rev 1 of this spec (git
history) holds the worked design for that layer.

## 6. Out of scope entirely

- Def-unloading on detach / project switch (eseq-jo7.21's problem; the
  import lines in scratch are the attach list that fix will want).
- Version selection for package deps; remote registry.
