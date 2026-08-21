# Module System — Namespaces, Imports, and Packages for eseqlisp

Status: rev 3, 2026-08-11 — surface syntax locked (`module` / `import` /
`/` qualifier / explicit exports); slice 1 scoped with the sdf stdlib conversion
as the acceptance test; §6.1 adds `override` (advice-style, survives owner
reload) as the blessed user-override mechanism. **§6 hooks are BUILT** ahead of the module slices:
`defhook`/`add-hook`/`remove-hook`/`run-hook` natives in
`crates/eseqlisp/src/lang/vm.rs` (registry `VM::extension_hooks`,
snapshot-aware), the four `macro-mapping-*-hook` stubs converted, unit
tests `extension_hooks_*` in vm.rs. Everything else unbuilt.

## 1. Motivation

The UI lisp is ~21.6k lines with 1,854 flat global definitions, and ~95% of
them already follow a prefix-as-namespace discipline enforced by nothing:
`seqv-` (244 defs), `seq-` (150), `sbrowser-` (142), `arrangement-` (126),
`mixer-v2-` (113), `piano-roll-` (55), plus a second family set under
`ui/effects/` (`fx-` 163, `instrument-` 122, `rack-` 56). The prefixes are
doing package work by hand, which is why names run to 48 characters
(`seq-open-arrangement-piano-roll-bottom-for-track`) and why the flat table
silently eats real bugs today — `sequencer-cursor-step-changed` is defined in
both `ui/main.lisp:1059` and `ui/sequencer.lisp:256` with last-load-wins, and
`instrument-param-base-value` is defined twice in the same file
(`ui/effects/param-controls.lisp:287` and `:751`).

Beyond cleanup, this is the load-bearing infrastructure for the extension
story: third-party packages, distros, and shareable snippets all need (a)
collision-proof names with visible provenance, (b) a declared dependency
mechanism, and (c) a first-class hook seam so extensions don't rely on
load-order-fragile redefinition.

Terminology, fixed: a **module** is one file with one namespace (the unit of
loading, hot reload, and qualification). A **package** is the distribution
unit — a directory with a manifest containing one or more modules. The words
are never interchanged (Common Lisp's mistake).

## 2. Surface syntax (locked)

```lisp
;; ui/mixer.lisp
(module eseq.mixer)

(import eseq.track-collapse :as tc)
(import eseq.seqv :refer (cursor-step))

(export track-strip)

(defstate panel-visible false)

(def track-strip (i)
  (if (tc/track-collapsed? i) (collapsed-strip i) (full-strip i)))
```

Locked decisions:

1. **`(module <name>)`** — a top-level declaration, not a wrapper. Everything
   after it in the file belongs to that namespace. One `module` form per
   file; file = module = hot-reload unit, matching the Rust side's existing
   `ModuleRecord`/`ModuleGraph`/`eval_module_source` vocabulary
   (`crates/eseqlisp/src/hot_reload.rs:39-50`,
   `crates/eseqlisp/src/lang/vm.rs:3312`). Rejected names: `ns` (Clojure
   jargon), `namespace` (collides verbally with reactive namespaces),
   `package` (reserved for the distribution unit), `in-package` (stateful
   phrasing), `provide` (means something else to Emacs users).
2. **Separate top-level `import` lines**, not a clause nested in the module
   form. One line = one dependency: diff-friendly, tooling can insert/sort
   lines, a pasted snippet carries its own import, and the identical form
   works at the REPL (no Clojure-style file-vs-REPL split). `:as <alias>`
   binds a module-local alias; `:refer (<sym> …)` pulls named symbols in
   bare, explicit and per-symbol only. `import` inside a function body is an
   error; imports may appear anywhere at top level, the linter nudges them
   to the top.
3. **`/` is the qualifier** and becomes reader syntax: `tc/track-collapsed?`
   splits at the first `/` into namespace-or-alias + name. A symbol that is
   only `/` remains the division builtin (Clojure's rule). The name part may
   not contain further slashes. `.` is unavailable — it is taken by reactive
   namespaces (`SEQ.x`, single-dot enforced at
   `crates/eseqlisp/src/lang/compiler.rs:643-654`).
4. **Named modules are private by default and declare their public surface
   with `(export …)`.** This supersedes the migration-era `%name` privacy
   convention. Qualified access to a non-exported symbol remains callable but
   warns, while `:refer` of one is an error. The complete syntax, semantics,
   and rationale are specified in [`module-export-spec.md`](module-export-spec.md).
5. **Function syntax is `(def name (args) body)`** — this spec's examples
   use the real eseqlisp form (e.g. `(def track-peak (i) …)`,
   `ui/mixer.lisp:6`), not Scheme-style `(def (name args) …)`.

## 3. Resolution semantics

Bare reference inside a module resolves in order: lexical scope (locals,
upvalues — unchanged) → current module → `:refer`red symbols → core prelude.
Qualified reference `X/name`: `X` resolves as an import alias first, then as
a full module name. Severity of an unknown `X` splits by shape (decided
during S1, ratified 2026-08-12): an **undotted** namespace is alias-shaped
(`tc/foo` with no import binding `tc`) — almost certainly a typo or missing
import — and is a **hard compile error** at load time, not a runtime
surprise. A **dotted** namespace reads as a full module name and only
**warns once** when unknown: the cross-module escape hatch below must stay
load-order-independent (defining into a module that has not been evaluated
yet is legal during migration).

Implementation is name mangling at the existing choke points, nothing more:
`resolve_symbol` (`compiler.rs:1095`) and `ensure_global` (`vm.rs:3506`)
intern `eseq.mixer/track-strip` as an ordinary string in the existing flat
`Vec<String>` global table. The VM does not change. `(def eseq.seqv/foo …)`
— defining into another module explicitly — is legal as a migration escape
hatch, warned as off-the-paved-road.

**Core namespaces.** A short blessed list (`eseq.core` — the natives and
builtin widgets — and `sdf`) is always resolvable, bare or qualified, with no
import. Rust natives register into namespaces via a namespaced variant of
`register_native_with_vm` (`vm.rs:3200`); a module's bare `def` may not
shadow a core name without a warning (today `(def label …)` silently shadows
the builtin `label` widget).

**Macros.** `Compiler::macros` (`compiler.rs:1484`, mirrored in
`VM::macros`) is currently a flat `HashMap<String, MacroDef>`; lookup becomes
namespace-aware with the same resolution order. This is required in slice 1,
not later — the sdf stdlib is macros.

**Cross-module redefinition** (monkey-patching another module's symbol)
stays legal and warned. It is a migration escape hatch only — users who
*want* to replace another module's definition should use `override` (§6.1),
which expresses the same power as intent, survives owner re-evaluation, and
does not warn.

## 4. `import` vs `load`

`import` = load-once + alias registration. It resolves a module name to a
file (§7), evaluates it if and only if it has not been evaluated *this
import pass*, and records the dependency edge in the existing `ModuleGraph`
(edges currently inferred from observed `load`s at `hot_reload.rs:282-290`
become declared). This dissolves the hand-maintained ordering in
`ui/main.lisp` (the "define before loading render roots" comments, and
`track-collapse.lisp` being raw-loaded three times from `browser.lisp:5`,
`mixer.lisp:4`, `sequencer.lisp:5`).

**`import` has a compile-time half** (eseq-mods.12, resolving §10 hazard
(p)): a compile unit is split at its top-level `(import …)` forms and
compiled/executed segment by segment (`VM::eval_str`,
`split_at_top_level_imports`). Each import therefore evaluates its target —
through the ordinary `__import-module` per-pass ledger — *before any later
form in the same unit compiles*, and the next segment's compiler is
re-seeded from the VM with the target's `defstate` keyspace, macros and
compat-alias spellings (`Compiler::new_repl` seeding), while the unit's own
module identity (`(module …)` declaration, `:as`/`:refer` bindings) threads
across segments via `Compiler::take_module_context`. A unit with no
top-level imports compiles in one segment, exactly as before. Consequences:

- An import supplies both runtime AND compile-time surface: the old rule
  "compile-time dependencies are ordered by the loader" is retired — a
  module that needs another module's `defstate`s or macros imports it.
- Only a **literal top-level** `(import …)` form has the compile-time half;
  quoted or nested occurrences (already a compile error inside functions)
  stay runtime-only.
- Load-once still holds: a target already evaluated this pass is only a
  re-seed, not a re-eval, so a REPL `(import …)` is cheap and hot reload's
  per-pass re-arm semantics are unchanged.
- Failure mid-unit: a compile error in a later segment surfaces after
  earlier segments (including their imports) executed. The transactional
  entry points roll the whole pass back via their snapshot; a bare
  `eval_str` keeps the earlier segments' effects, matching the precedent
  that load-once side effects persist across a failed `load`.
- Cycles terminate exactly like the runtime path: the importer's
  `(module …)` declaration executes in its first segment, before its
  imports run, so a back-import is a ledger hit. The back-importer compiles
  against the partial surface the cycle target has executed so far —
  declare the module before the imports (the standing convention) or a
  cycle can double-evaluate the importer.

`load` survives unchanged as the raw evaluate-this-file-here primitive.
Themes require re-evaluation semantics (`seq-apply-theme-file` loading a
file IS applying the theme), and a distro manifest may use it for explicit
assembly. Rule of thumb: `import` for code you call, `load` for files whose
evaluation is the side effect you want.

## 5. Registries auto-qualify

The symbol table is only one of several flat string keyspaces. Every
registration form prefixes its name with the current module unless the name
is already qualified:

- `defstate` → `eseq.mixer/panel-visible` in `state_bindings`
  (`vm.rs:1815`).
- Widget `:key` → the string is prefixed before the FNV hash
  (`crates/eseqlisp/src/ui/layout.rs:1904-1921`), so
  `:key (str "cell-glyph-" i)` in `eseq.mixer` hashes as
  `"eseq.mixer/cell-glyph-3"`. Collisions between two panels' generic keys
  become impossible by construction.
- `define-mode` names and `def-process` names, likewise.
- **Late-bound string handlers capture their module.** `bind-key` handler
  strings (`crates/eseqlisp/src/editor/natives.rs:69-83`) and mode `:on-key`
  strings (e.g. `ui/mixer.lisp:1349`) are resolved at dispatch time; the
  binding record stores the module current at binding time and resolution
  qualifies against it. Without this, mangling silently breaks every keymap.
  (Longer term, prefer passing function values over name strings — the
  string indirection is a rename-breaks-silently bug independent of this
  spec.)
- Namespaced keywords: `:eseq.mixer/mode` is legal keyword syntax with the
  same first-slash split, and `::mode` expands against the current module.
  Extension-written data in serialized projects should use them so two
  extensions stashing `:mode` on a track can never collide.

**Override identity caveat.** An `override` body (§6.1) evaluates in the
*overriding* module, so auto-qualified `:key`s and `defstate`s inside it
become `alec.init/…`, not `eseq.mixer/…` — the overridden component gets
fresh reactive state rather than inheriting the factory component's. That
is the correct default (it *is* a different component), but anything
serialized against those keys (p-locks, layout state) changes identity when
an override lands or is reverted. Deliberate, documented, revisit if it
bites.

Reactive namespaces (`SEQ.x`, `THEME.y`) are explicitly out of scope: they
are a field-access mechanism, not symbol packaging, and keep their dotted
syntax untouched.

## 6. `defhook` — replacing stub-then-override

**Naming note:** eseq has a prior, unrelated hook system —
`register-hook`/`clear-hooks` (`crates/eseqlisp/src/mode.rs:93`,
`crates/sequencer/src/app/effects.rs:242-283`), musical-clock callbacks
(`:step`/`:beat`/`:bar` + interval + track,
`HookUnit`/`register_control_hook`). **Locked decision (2026-08-11): that
system is deprecated for removal** — an early experiment that didn't earn
its keep — so this feature owns the word "hook" outright, matching what
Emacs-literate users expect it to mean (extension point). Removal is
independent of this spec's slices; until it lands, the old builtins simply
coexist.

The stub-then-override idiom (define a global, let a later file redefine
it) is used in two directions today, and both depend on load order and
last-writer-wins:

- **Extension points**: `macro-mapping-*-hook` no-op stubs in
  `ui/macro-state.lisp:8-30` are overwritten by `ui/main.lisp:713-722` and
  `ui/effects/param-controls.lisp:47`.
- **Host↔script protocol**: `ui/main.lisp:241-249` stubs
  `script-buffer-name`/`script-tab-label`/`script-sequencer-name`/
  `script-init-fn`; each sequencer script (e.g.
  `scripts/sequencers/graph-neural-variable-reset-demo.lisp:91-96,240`)
  overrides them and the script picker reads them back / calls
  `script-init-fn` after load. Consequence: only one "current script" can
  exist — a second load clobbers the first's protocol globals.

Strict modules would break both, so each gets a first-class replacement.
Extension points become hooks — **BUILT 2026-08-11** (pre-module, so hook
names are flat strings for now; slice 3's registry auto-qualification
covers them like every other string key):

```lisp
;; in macro-state.lisp — declares the hook AND defines a global function of
;; the same name that runs its listeners, so call sites stay ordinary calls:
(defhook "macro-mapping-sidebar-open-hook")

;; in main.lisp — the entry key makes re-evaluation (hot reload) replace
;; instead of duplicate; a re-added key keeps its position in run order:
(add-hook "macro-mapping-sidebar-open-hook" "seq-shell"
  (lambda () …))
```

Semantics as implemented (`crates/eseqlisp/src/lang/vm.rs`,
`register_core_natives`; registry `VM::extension_hooks`, included in
`snapshot_state`/`restore_state` so transactional evals roll back cleanly):
listeners run in registration order; a listener error is logged (gated by
`ESEQLISP_DEBUG_LISP_ERRORS`) and does not stop the rest; hooks return nil —
**callers must not depend on listener return values** (the
`macro-toggle-mapping-arm` conversion made its `true` return explicit for
exactly this reason). `(run-hook "name" args…)` runs a hook by name with
arguments forwarded to every listener; `(remove-hook "name" "entry-key")`
unregisters; running a listener-less hook is a no-op, which replaces the
old no-op stubs. Multiple listeners instead of last-writer-wins, no
load-order dependence, and the published hook list becomes the de facto
extension API surface. All four `macro-mapping-*-hook` stubs are converted
(macro-state.lisp declares; main.lisp listens under key `"seq-shell"`,
param-controls.lisp under `"param-controls"`). Unit tests:
`extension_hooks_*` in vm.rs.

### 6.1 `override` — advice, not redefinition

**BUILT 2026-08-13 (slice 4).**

Hooks cover extension points the factory *anticipated*. The other half of
the extension story is replacing a definition the factory did not
anticipate anyone touching — a user swaps in their own step toggle, track
strip, or mixer channel from `~/.eseq.d/init.lisp`. The naive spelling is
the §3 escape hatch, `(def eseq.mixer/track-strip …)` from the user's file.
That works until the owning module re-evaluates (hot reload of
`mixer.lisp`, theme re-apply, any mid-session re-eval) and clobbers the
user's def — the same load-order fragility §6 exists to kill.

Emacs's lesson: advice survives redefinition **because it is stored on the
symbol, not in the function value**. Steal that:

```lisp
;; ~/.eseq.d/init.lisp — full replacement:
(override eseq.mixer/track-strip (i)
  (my-neon-strip i))

;; wrapping, with the factory definition in scope:
(override eseq.mixer/track-strip :around (original i)
  (badge-wrap (original i)))
```

Semantics:

- Overrides live in a registry (`VM::overrides`, keyed by qualified name),
  structurally a sibling of `VM::extension_hooks` — included in
  `snapshot_state`/`restore_state`, one entry per (symbol, overriding
  module), last-write-per-module-wins so hot-reloading the user's init file
  replaces rather than stacks. If multiple modules advise one symbol, the
  most recently evaluated registration is active; `remove-override` removes
  the symbol's advice set and returns directly to factory behavior.
- Global **reads check the override registry before returning the owning
  def**. This check is in `VM::global_read_cell`, reached by cached
  `LoadGlobal` indices as well as host by-name reads; a compiler-resolution
  rung alone could not intercept chunks compiled before registration. The
  no-override hot path is one `HashMap::is_empty()` branch and performs no
  lookup or allocation. The factory cell is never mutated, so owner-module
  re-evaluation refreshes the definition underneath without disturbing the
  advice. Load order stops mattering.
- `:around` receives the *current* underlying def as `original` at call
  time (not captured at override time), so the wrapper composes with
  factory updates.
- `(remove-override eseq.mixer/track-strip)` is "revert to factory." The
  inspector can show provenance: *track-strip — overridden by
  ~/.eseq.d/init.lisp*.
- **Graceful failure:** an override whose body errors at call time emits one
  diagnostic, quarantines that registration, and falls through to the
  factory def. Later reads bypass the broken body until it is re-registered,
  so a per-frame UI call cannot repeat the error indefinitely. A broken user
  override degrades one component; it never bricks the app.
- Overriding a non-exported symbol warns, exactly like a qualified reference
  to one (§2, decision 4): internals are the unstable rung, and the warning
  enumerates which overrides may break on update. Overriding exported defs is
  the semi-stable API surface.

`metal_seq` discovers the user entrypoint at `$ESEQ_CONFIG_DIR/init.lisp` when
that test/development override is set, otherwise at `~/.eseq.d/init.lisp`.
It evaluates the file **last**, after the distro root, custom instrument/effect
UI, and project scratch content. Missing paths are silent. Evaluation is
transactional: an error restores factory state, logs the failure, opens
`*lisp-reload*`, and leaves boot running. Successful and initially-erroring
existing init files are included in the external-path file watcher; saves are
re-evaluated transactionally, so user advice and the factory definitions below
it both hot-reload. Path-associated evaluation goes through
`VM::eval_module_source`, so the durable old-spelling detector from
eseq-mods.13 applies without an init-specific path.

The recommended escalation ladder for users, each rung trading power for
update-fragility: **customize** (`defcustom`, theme slots) → **extend**
(`add-hook`, new buffers, keybindings) → **override** (this section — one
component, survives updates) → **shadow** (your own module file earlier in
the load path, §7 — whole-module replacement, distro territory).

**Well-known-name protocols resolve per-module.** The host↔script protocol
needs no stubs at all under modules: the loader knows which module it just
loaded, so it looks up the conventional names *inside that module's
namespace* (`alec.neural-demo/script-init-fn`,
`…/script-buffer-name`). Script authors write exactly what they write today
— bare `(def script-init-fn () …)` under their `module` header — the
`main.lisp` stubs are deleted, and N scripts coexist because each script's
protocol symbols live in its own namespace. The same pattern applies to any
future "host calls into a user file by convention" seam.

## 7. Module → file resolution and the load path

Convention: module name maps to path segments, `eseq.track-collapse` →
`track-collapse.lisp` under a load-path root. Search order:

1. app-bundled core (`crates/sequencer/ui/`, the vanilla distro),
2. `packages/<pkg>/src/`,
3. user dir `~/.eseq.d/`.

A distro is an entry earlier in the search path that can shadow how a module
resolves. This generalizes the `@/` prefix convention
(`hot_reload.rs:266-281`) into a real mechanism.

**init.lisp inversion.** Today `init.lisp` is evaluated **before**
`ui/main.lisp` into the same table
(`crates/sequencer/src/ui/editor_setup.rs:33` precedes `:44`), so the UI
clobbers the user — backwards. With modules: core loads, then distro, then
`~/.eseq.d/init.lisp` **last**, so the user always wins. A `$HOME` lookup is
added to `eseqlisp_init_candidates` (`crates/sequencer/src/paths.rs:29-50`),
which currently has none.

## 8. Packages

Generalize the existing dgenlisp defmacro-library format
(`crates/eseqlisp/src/defmacro_library.rs` — dir-per-package,
`manifest.json` with a version field, `use-defmacro` imports with cycle
detection and local shadowing), relaxing its one-public-symbol rule:

```
packages/alec.acid-tools/
  manifest.json        ;; name, version, deps, entry module, external assets
  src/*.lisp           ;; modules under alec.acid-tools.*
```

- **Author-scoped names**: `alec/acid-tools` owns modules under
  `alec.acid-tools.*`. Collision-proof by construction; provenance visible
  in every qualified symbol and in the inspector.
- `eseq.*` and all single-segment module names reserved for core, forever.
- **Version lives in the manifest, never in a module name** (`mixer-v2-` is
  the cautionary tale, 113 defs deep). Registry policy: breaking change =
  new name; otherwise only accrete.
- The manifest declares external assets (samples, IRs) by name + hash so a
  package can report what's missing instead of silently loading broken.
- `defcustom` (declared knobs: type, default, docstring, auto-qualified)
  ships with the package layer; the settings UI is generated from the
  declarations since the UI is lisp.
- **Package exports need no loader coordination** (unlocked by import's
  compile-time half, §4): a package module can export macros and
  `defstate`s, and a consumer's `(import alec.acid-tools.ui)` supplies
  them at the consumer's own compile time — no distro-root ordering, no
  "publishers must be root entries" rule. This is what makes third-party
  packages composable: the loader never has to know what a package
  publishes.

## 9. The sdf pilot (slice 1 acceptance test)

`sdf-stdlib.lisp` already hand-rolled this exact convention as flat strings:
~17 defmacros literally named `sdf/circle`, `sdf/rounded-rect`, `sdf/rotate`
etc. (`crates/eseqlisp/sdf-stdlib.lisp`, loaded at `Runtime::new` via
`include_str!`, `runtime.rs:1245`), plus `sdf/layer`/`sdf/fill`/`sdf/paint`
as Rust-registered builtins referenced from lisp-in-Rust template strings
(`runtime.rs:572-600`, `lib.rs:2190`). ~34 lisp files consume `sdf/*`.

Conversion: the file gains `(module sdf)` and the defmacros drop their
prefixes; internal fill-shape macros (`__hslider-fill`,
`__vslider-fill-with-material`) become unexported `hslider-fill` etc.; the Rust
builtins register into the `sdf` namespace. **Every call site keeps working
verbatim** — `(sdf/circle val)` now parses as qualified reference instead of
a lucky flat string. If all 34 consumers and the Rust templates pass
untouched, slice 1 is proven before any `seqv-` symbol moves.

The pilot forces the two runtime slices that must not be deferred:
namespace-aware macro lookup (§3) and namespaced native registration (§3),
because sdf exercises both.

Note: the double-underscore idiom inside sdf expansions (`__rot_cos`,
`__lit_n`) is fake gensym hygiene, a separate concern from privacy — it
stays as-is; real macro hygiene is out of scope for this spec.

## 10. Migration

- **Slice 0 — implicit `eseq.vanilla`.** Headerless files compile as module
  `eseq.vanilla`. Zero behavior change; the whole current app becomes one
  big module and "vanilla is just a distro" becomes literal.
- **Slice 1 — reader + resolution + sdf pilot.** `/` split, `module`,
  `import`, `%` warning, namespace-aware macros, namespaced natives,
  sdf-stdlib conversion (§9).
- **Slice 2 — split `ui/main.lisp`.** It is a load manifest plus four
  modules in a trenchcoat (`seq` shell commands, 51 stray `seqv-` defs,
  `bus-`/`step-` plumbing, macro-hook overrides), sharing 67 symbols with
  `sequencer.lisp` including a back-edge cycle (e.g. `cool-off-follow`
  defined `main.lisp:71` called from `sequencer.lisp`;
  `seqv-collapse-all-tracks` defined in `sequencer.lisp` called from
  `main.lisp`). The split must precede per-file headers or the headers lie.
  `param-mode` (`main.lisp:24`, mutated 35× from `step-grid.lisp`) gets an
  owning module.
- **Slice 3 — per-file `module` headers + compat aliases.** One file at a
  time; prefix families make renames mechanical
  (`mixer-v2-track-pattern-cell-selected-binding` →
  `eseq.mixer/pattern-cell-selected-binding`). A compat alias table (old
  flat name → qualified) keeps unconverted callers working mid-migration
  and is deleted at the end. Registry auto-qualification (§5) lands here,
  per-file with its module header. **Stage 1 (BUILT 2026-08-12):** the
  infrastructure — auto-qualification gated on declared modules (vanilla
  files keep flat keys, so no serialized identity shifts until a file
  converts), chunk-level module provenance (`Chunk::source_module`,
  `VM::current_module_name`) for late-bound registration natives, and the
  alias mechanism: a converted file declares
  `(module-compat-alias old-flat-name new-name)` (top level; `new-name`
  qualifies against the current module when bare). Aliases live in
  `VM::compat_aliases` (snapshot-aware), are consulted by both resolution
  ladders on a bare name — ahead of the implicit-module/flat entries so a
  stale pre-conversion `eseq.vanilla/` slot cannot shadow the new home,
  behind a declared module's own entry and `:refer`s — and apply to reads
  and writes (an unconverted redefiner keeps last-writer-wins against the
  new home). Deleting the table and the form ends the migration.

  **Stage 2 (BUILT 2026-08-12, batch 2):** `module-compat-alias` covers the
  **macro table** as well, no new form — `Compiler::lookup_macro` gained the
  same alias rung in the same ladder position (current module → `:refer` →
  alias → flat), because macros are a third flat keyspace and a converted
  file's renamed macros would otherwise strand every unconverted caller.
  Macro *definition* sites do NOT follow the alias (unlike global writes,
  which do): several standalone demos and Rust test fixtures define their own
  flat `aqua-color`/`aqua-slider-material`, and write-through would let those
  clobber a converted module's macro. The consequence is that a vanilla
  redefinition of an aliased macro name is silently ignored in a VM where the
  alias exists — acceptable because the alias table is migration-only, but it
  means an alias should not be minted for a name that unconverted files
  redefine on purpose. Shader/material bodies expand in a throwaway
  implicit-module compiler (`runtime::expand_sdf_expression`), which is now
  seeded with the alias table too.

  **Stage 3 (BUILT 2026-08-12, batch 3): the late-binding heal reaches
  through compat aliases**, which retires the load-order gate for globals.
  `VM::late_bind_empty_global` heals an empty global slot on its first read;
  it now consults `VM::compat_aliases` as its first fallback rung, so a
  caller compiled *before* a file's conversion — which interned
  `eseq.vanilla/old-name`, or flat `old-name` if the name happened to be
  interned flat already — resolves to the converted module's cell instead of
  erroring. The ladder is deliberately ordered and mirrors
  `resolve_global_read_index`:

  1. exact (already known empty — that is why the heal ran),
  2. **compat alias on the bare base name.** `module-compat-alias` validates
     its old name flat, so alias keys are never qualified: the base of a
     stale `eseq.vanilla/old-name` slot and a stale flat `old-name` slot
     reduce to the same lookup key, and there is no spelling for which
     looking the *full* name up as an alias key could hit. The alias rung
     sits ahead of the implicit/flat rungs for the same reason it does in
     both resolution ladders — a stale pre-conversion vanilla slot must not
     shadow the new home,
  3. the implicit-module spelling `eseq.vanilla/<base>`,
  4. the flat spelling `<base>`.

  Rungs 3–4 apply only to a qualified stale slot; a flat empty slot heals
  through the alias alone, because healing flat → `eseq.vanilla/…` would
  cross the reactive-namespace flat exemption in
  `resolve_global_read_index`.

  The heal *aliases the slot to the found cell*, so a `StoreGlobal` to the
  stale index replaces the slot `Option` and unlinks it — write-then-read
  keeps last-writer-wins. Caveat, tested: a pre-conversion reader and a
  pre-conversion writer share one stale index, so once the writer fires the
  pair stops tracking the module's own value. The heal is a read-side
  rescue, not two-way aliasing.

  **Asymmetry with macros.** The heal is a *runtime* mechanism keyed on an
  empty global slot; macros expand at compile time, so nothing analogous can
  exist for them. `compat_alias_macro_does_not_retrofit_an_earlier_compiled_caller`
  stays as-is and pins that: **a converted file's macros still need the
  step-0 load-order gate**, its globals no longer do. Tests:
  `late_binding_heals_earlier_compiled_caller_through_alias`,
  `late_binding_heals_a_stale_flat_slot_through_alias`,
  `late_binding_without_an_alias_keeps_the_old_behavior`,
  `a_store_through_the_stale_slot_unlinks_the_heal` (vm.rs).

  ### Slice 3 conversion recipe (validated by batch 1, 2026-08-12)

  Batch 1 converted `ui/macro-state.lisp` → `eseq.macro-state`,
  `ui/seq-macro-mapping-hooks.lisp` → `eseq.seq-macro-mapping-hooks`, and
  `ui/choose-model.lisp` → `eseq.choose-model`. The steps below are what
  that batch had to do; later batches follow them mechanically.

  **Step 0 — the load-order gate. RETIRED FOR GLOBALS (stage 3, batch 3);
  still live for macros.** Historically a compat alias only helped callers
  that compiled *after* it was evaluated: `(load …)` runs at the *runtime*
  of the loading file and a file compiles in full before it runs, so
  "consumer.lisp `(load "dep.lisp")` on line 5" was NOT early enough — the
  consumer had already interned `eseq.vanilla/dep-fn` and emitted an index
  for it. The stage-3 late-binding heal now retrofits exactly that slot on
  its first read, so **a def-only file converts regardless of load order**,
  and the track-collapse-style hoist (moving a self-loaded dep up into the
  manifest, `964a4d40`) is no longer required. The hoists already landed
  stay — they are independently correct — but new conversions do not need
  them.

  What still needs the gate: **macros**. The heal is a runtime mechanism
  keyed on an empty global slot, and macro expansion happens at compile
  time, so a caller that compiled before an aliased macro existed is
  permanently stranded (`compat_alias_macro_does_not_retrofit_an_earlier_compiled_caller`).
  If the file defines macros with external callers, list every path that
  evaluates it — production manifests, other lisp files, Rust test harnesses
  that `eval_str`/`load` a subset — and confirm it is evaluated before each
  consumer compiles. Macros reached only through auto-quoted
  `:shader`/`:material` values are exempt (hazard h — they expand at render
  time).

  The reverse direction is also healed now: a converted module's own bare
  references *out* to unconverted globals intern a qualified slot that stays
  empty until the vanilla def lands, and the first read heals it
  (`module_forward_reference_to_later_vanilla_def_late_binds`).

  **Step 1 — header and renames.** Add `(module eseq.<basename>)` after the
  file header comment; the module name must match the filename so §7
  resolution works later. Drop the hand-rolled prefix where it duplicates
  the module concept (`macro-mapping-selected` → `mapping-selected` in
  `eseq.macro-state`), keep internal callers bare, and `%`-prefix helpers
  with no callers outside the file (`%body`, `%options`).

  **Step 2 — one `module-compat-alias` per renamed name with an external
  caller**, immediately after the module form. "External caller" includes
  Rust-generated lisp source (e.g. the patcher buffer templates in
  `src/ui/edit_sessions.rs`) and tests that drive the file by name. A name
  that does *not* change spelling still needs an alias if unconverted
  callers reference it bare — `(module-compat-alias choose-model choose-model)`
  — because bare `choose-model` does not find `eseq.choose-model/choose-model`.

  **Step 3 — hazard checklist** (each item below fired at least once, or was
  the reason a candidate was dropped):

  a. **Widget `:key` re-keys.** Keys auto-qualify, so `:key "dropdown"` in
     `eseq.choose-model` hashes as `eseq.choose-model/dropdown`. Layout,
     focus, and `find_layout_node_by_stable_key` assertions must be
     requalified. Do not convert a file whose keys reach serialized state
     (p-locks, saved layout) without a migration.
  b. **`defstate` is a second keyspace.** Auto-qualifying a `defstate`
     shifts `state_bindings`, and the alias must be honoured there too or an
     unconverted `(set! old-name v)` resolves the *global* through the alias
     while missing the binding — it then stores over the slot holding the
     `NodeRef` and fails with `IncorrectType`. Batch 1 hit this immediately;
     the alias ladders in `Compiler::state_binding_for` and
     `VM::state_binding_node` now cover it (`compat_alias_covers_the_defstate_keyspace`).
  c. **`defwidget :state` sets** are bare-keyed at shader compile time even
     though runtime reads ladder — keep widget state names inside one file.
  d. **`(current-buffer-mode)` and mode names** qualify: lisp comparing
     against flat mode strings breaks. None in batch 1. **Batch 3 vetoed
     `ui/seq-grid-mode.lisp` on this hazard** — it is the reason the mode
     keyspace is the next piece of infra, not a conversion. `define_mode`
     qualifies the mode name *and* its `:on-enter`/`:on-key` handler strings
     (`runtime.rs`), and `mode_bind_key` / `set_buffer_mode_for` qualify
     their mode argument against the *caller's* module — but
     `Editor::resolve_mode_name` ladders qualified → flat only, with **no
     flat → qualified rung and no compat-alias rung**. So the moment
     `(define-mode "seq-grid-mode" …)` moves into a module, the live vanilla
     call `(set-buffer-mode-for "*sequencer*" "seq-grid-mode")` in
     `sequencer.lisp` stops resolving — the `*sequencer*` buffer loses its
     keymap, silently. Second, unfinished thread: `mode_bind_key` qualifies
     the handler string unconditionally, and this file binds seven handlers
     that live *outside* it (`cursor-left`, `cursor-right`,
     `select-all-steps`, `delete-selected-steps`, `cursor-toggle`,
     `seqv-collapse-all-tracks`), so the handler ladder's qualified → flat
     fallback needs its own test before any mode-defining file converts.

     **Stage 4 (BUILT 2026-08-12, batch 4 infra) — the mode keyspace has its
     alias rung.** `Editor::resolve_mode_name` now ladders exactly like the
     global / `defstate` / macro ladders, on the same keyspace-agnostic
     `VM::compat_aliases` table (reached from the editor through the new
     `Runtime::compat_alias_target`, since the mode registry lives outside
     the VM):

     1. **exact** — the referencing module's own mode, or a vanilla flat one;
     2. **compat alias on the bare base name.** Alias keys are validated flat
        at record time, so a flat reference and a caller-module-qualified one
        reduce to the same key. This *is* the flat → qualified direction:
        `(module-compat-alias seq-grid-mode seq-grid-mode)` in the converted
        file is what keeps the unconverted
        `(set-buffer-mode-for "*sequencer*" "seq-grid-mode")` working. Ahead
        of the flat rung, as in every other ladder;
     3. **flat base** — qualified → flat, the pre-existing rung: a module
        referencing a *vanilla* mode.

     There is deliberately **no** "scan the registry for any key whose base
     segment matches" rung — ambiguous across modules, and it would resolve
     by hash order. Minting the alias (recipe step 2) is the declared route.

     The second thread is closed too: `mode_bind_key`'s unconditional handler
     qualification is safe because dispatch runs the stored string through
     `Runtime::resolve_handler_name` (qualified → flat when the module never
     defined it), verified end-to-end by
     `module_mode_binding_dispatches_a_vanilla_handler` — a module defines a
     mode, binds a key to a handler defined in vanilla, and the key press
     dispatches. Tests (editor/tests.rs):
     `compat_alias_reaches_a_module_defined_mode_from_a_flat_caller`,
     `module_mode_reference_falls_back_to_a_vanilla_mode`,
     `module_mode_binding_dispatches_a_vanilla_handler`.

     Still un-covered by any rung, and still a per-file check: lisp that
     **compares** `(current-buffer-mode)` against a flat string literal. The
     alias table maps names in the *resolution* direction only; a converted
     mode reports its qualified spelling to lisp. Grep the mode name before
     converting.
  e. **Flat keyspaces that do NOT qualify**: hook names (`defhook`/`add-hook`
     strings), `defchan`, subtree keys, `defwidget` names. Leave those
     strings alone. **Subtree keys, precisely (batch 4):** `(subtree :key …)`
     compiles to a `subtree-owner` call (`compiler.rs:compile_subtree_form`)
     whose key goes straight to `explicit_subtree_root_hash` and never passes
     through `VM::qualify_widget_stable_key`. So a converted file's `:key`
     props split into two keyspaces that must be treated oppositely: **widget**
     `:key`s qualify and should drop the file's hand-rolled prefix (the
     qualifier supplies the provenance), while **subtree** `:key`s stay
     byte-identical — stripping them would put a bare `track-0` into a flat
     *global* keyspace shared with every other file. The same split governs the
     test assertions: widget-key sites move to the `/`-suffix matcher, subtree-key
     sites keep their exact flat spelling. Corollary discovered in batch 1: `defhook` registers its
     caller-facing native *at runtime* under the flat name, so a converted
     module cannot call `(the-hook-name)` bare — its own call site compiles
     before that global exists and interns a dead qualified slot. Inside a
     module, invoke hooks as data: `(run-hook "the-hook-name")`.
  f. **Name surfaces show the qualified spelling.** M-x candidates and
     completions come from `global_names`, so a converted command lists as
     `eseq.choose-model/choose-model` (still filtered by typing
     `choose-model`, and still callable bare through its alias). Alias old
     names are also completion/M-x candidates (`completion_symbols` includes
     the alias table's keys).
  g. **Module defs colliding with builtin widget names.** A module `def`
     never clobbers the flat native (it interns qualified, with a warning),
     and the source-annotation pass knows a unit's own defs shadow widget
     names — without that, `(select v)` in `eseq.choose-model` was
     annotated with `__source-*` widget props and called the 1-arg module
     fn with 4+ args (`ArityMismatch` on every dropdown pick). Both are
     handled in the infrastructure now, but prefer non-colliding def names
     anyway: the def-site shadow warning firing during conversion is the
     signal to rename.

  h. **Auto-quoted `:shader` / `:material` bodies expand outside the
     module.** Both props are compiled with `compile_quoted_expression`, so
     the macros inside them are expanded much later, at shader-compile time,
     by a throwaway implicit-module compiler (`expand_sdf_expression`) — not
     by the compile of the file they are written in. Consequences, both hit
     converting `ui/materials.lisp`: a module's own shader bodies must
     reference its macros **qualified** (`eseq.materials/color`), because
     "current module" there is `eseq.vanilla`; and an unconverted consumer's
     shader body reaches the converted macros only through the compat alias,
     which is why the alias table is now seeded into that expansion path. The
     upside is that `:material` macro calls are late-bound: they resolve at
     render time, so they are exempt from the step-0 load-order gate.

  i. **Globals that Rust *generates lisp to write* stay in `eseq.vanilla`.**
     The stage-3 heal is read-side only: it repairs an *empty* slot, and a
     `StoreGlobal` fills one. So a pre-conversion **writer** is not rescued
     the way a pre-conversion reader is — it keeps storing into the stale
     `eseq.vanilla/` slot while later-compiled readers follow the alias to
     the module's slot, and the two silently diverge (no error, just a value
     frozen at its `def` initializer). This fired converting
     `ui/effects/state.lisp`: `crates/sequencer/src/ui/custom_ui.rs` emits
     `(def custom-synth-ui-… (inst) (do (set! synth-ui-current-inst inst) …))`
     into a generated unit whose compile time is not ordered against the
     file, and 23 `metal_seq` custom-UI layout tests eval that generated
     source *before* `ui/effects.lisp`. Fix: leave such names in the implicit
     module with the §3 cross-module def escape hatch
     (`(def eseq.vanilla/synth-ui-current-inst false)`) and mint no alias for
     them — they are a host→script protocol, not the module's API, and they
     fold in only when the Rust codegen is taught to emit qualified names.
     Rule of thumb: a name that Rust writes by bare spelling is not yours to
     move; a name Rust only *reads* by bare spelling is (aliases cover reads).

     **Pinning a `defstate` pins it in every keyspace (stage 5, batch 4).**
     `ui/browser.lisp` pins five reactive states, and the escape hatch did not
     originally reach the `defstate` registry: an explicitly-qualified
     registration name was passed through verbatim, keying `state_bindings` as
     `eseq.vanilla/<name>` while every flat reader and writer looks up `<name>`,
     with no implicit-module rung in either state-binding ladder — hazard (b)'s
     `IncorrectType` failure, reached through (i).
     `Compiler::qualify_registration_name` now **strips** an explicit
     `eseq.vanilla/` prefix, and `Compiler::state_binding_for` /
     `VM::state_binding_node` reduce the qualified spelling to the flat key on
     lookup (needed separately: without it a pinned state read *written*
     qualified compiles to `LoadGlobal` and returns the raw `NodeRef`). The
     principle is general — vanilla's registry keyspace **is** the flat keyspace
     under slice 0 — so it applies to `def-process` / `def-accumulator` names
     and any future registration form. Test:
     `a_vanilla_pinned_defstate_registers_flat` (vm.rs).

  j. **A module's bare *outbound write* needs its vanilla owner already
     defined** (found converting `ui/mixer.lisp`, batch 4 — the mirror image of
     (i)). `(set! some-vanilla-global v)` inside a module resolves through the
     usual ladder, so it finds the flat entry *if one exists at that point*.
     If none does, the write does not error and does not create the vanilla
     global: it lands in the module's own namespace, and the vanilla reader
     that appears later sees nothing. In a vanilla file the same `set!` creates
     the flat global on first write, which is why this only surfaces on
     conversion. Production is usually fine by load order — `ui/mixer.lisp`
     writes `sbrowser-loading-instrument-name`, and `ui/browser.lisp` loads at
     `main.lisp:17` against mixer's `:18`. Stub **test harnesses** are where it
     bites: they eval one file's source with none of its peers, and used to get
     the global for free from the converted file's own `set!`. Check the
     converted file's outbound `set!`s, confirm the owning file loads first in
     production, and declare the owner in any harness that does not load it.
     `ui/sequencer.lisp` makes the same write at `:381` and will need the same
     check.

  k. **The prefix strip can collide with a pre-existing *local* binding**
     (found converting `ui/arrangement.lisp`, batch 4). Dropping the
     hand-rolled prefix shortens every global to a name the file's own `let`
     heads, `lambda` args and `def` params may already use. Where that
     happens the rename does not error, does not warn, and does not change the
     local's meaning — it changes the meaning of the *global* reference that
     used to be distinguishable:

     ```lisp
     ;; before                                   ;; after the mechanical strip
     (let ((view-start (get event :view-start)))  (let ((view-start …))
       (if (= view-start nil)                       (if (= view-start nil)
         (+ arrangement-view-start delta)             (+ view-start delta)   ; ← now nil
         view-start))                                 view-start))
     ```

     The nil branch meant the module global; after the strip it reads the local
     that was just proven nil. It compiles and the whole test family stays
     green, because fixtures normally supply the field. **Required step:**
     after renaming, intersect the post-rename global names with every binder
     in the file (`def` params, `let`/`let*` heads, `lambda` args, `each`
     `|pipe|` vars). Any intersection is either a genuine shadow to rename
     (the arrangement fix renames the local to `event-start`) or a merged
     reference to audit. One hit in 128 defs, and no test caught it.

     **Widened by `ui/sequencer.lisp` (batch 4): also intersect against every
     global the rest of the app owns**, not just this file's locals and not
     just the vanilla names this file happens to reference. The strip can land
     on a name another file `def`s, and that is the worse case — it compiles,
     no test necessarily fails, and the collision silently redirects a call.
     Three shapes, all found in the same file:

     - the wrapper/delegate shape, 7 hits and the nastiest.
       `seqv-step-pointer-up` wraps the vanilla `step-pointer-up` and *calls*
       it; the mechanical strip merges the two into unbounded recursion. Any
       `<prefix>-<vanilla-name>` def is a candidate — the prefix was carrying
       the distinction.
     - a global this file reads (`cursor-step` vs `seqv-cursor-step`) — caught
       by the free-symbol intersection.
     - a global this file never mentions (`param-mode`, `current-step`,
       `current-page` in `seq-core-state.lisp`) — caught only by the app-wide
       sweep. `param-mode` is a vanilla **`defstate`**, so the module's `def`
       of the same base name was read back through `state_bindings` and every
       call died with `ExpectedFunction`: a module `def` must never collide
       with a vanilla `defstate` name.

     **Widened again by `ui/browser.lisp`: lisp globals are not the whole
     namespace.** Two of its six collisions were a registered **native**
     (`filter`, which that file calls three times) and a **builtin widget name**
     (`tabs`), neither of which appears in any `.lisp` `def`. The sweep set is
     four lists, not one: the file's own binders, every UI-lisp
     `def`/`module-compat-alias` name, the natives registered from Rust
     (`register_native*`), and `BUILTIN_WIDGET_NAMES` (`widgets.rs`). Two
     apparent collisions there are benign and should not trigger a rename: a
     name **pinned to `eseq.vanilla`** never strips, so it cannot collide with
     anything; and a name owned only by *already-converted* modules is qualified
     and has no flat entry to merge with.

     The practical recipe is one script: collect every `def`/`defstate`/
     `defwidget`/`defmacro` name in the headerless files plus every
     `module-compat-alias` key the converted modules publish, plus the native
     and builtin-widget lists, and intersect with the post-rename set. `%`-private names do not collide (`%` is part
     of the interned spelling), so the check only has to cover the public
     half plus any bare name the strip produces.

  l. **A lisp helper that returns a widget key for Rust to look up must emit
     the qualified spelling itself** (found converting `ui/sequencer.lisp`).
     Hazard (a) covers keys the *tests* assert on, but a key can also travel
     the other way: `seqv-current-number-picker-key` returns a stable key that
     `current_step_param_number_picker_key` (`src/ui/input.rs:762`) feeds
     straight into `layout_node_by_stable_key`, an exact match. Auto-
     qualification happens on the widget, not on the string the helper builds,
     so the helper now returns
     `"eseq.sequencer/expanded-param-number-picker-<id>"` with the module name
     written into the value. Grep a converting file for lisp that *constructs*
     a key rather than attaching one.

     **And write a test pairing the two (batch 4, `browser.lisp`).**
     `sbrowser-active-tree-key` is the second instance, and nothing in the suite
     covered it: the helper's spelling and the widget's `:key` are maintained in
     different places, and no ordinary layout assertion exercises both. The
     conversion added `metal_seq_browser_active_tree_key_matches_the_rendered_-
     tree_key`, which asserts the helper's return value locates the rendered
     node. Treat that pairing test as part of the hazard-(l) fix.

  m. **A module's bare reference to a mutable vanilla `def` global is frozen
     at its first read — and its bare write never lands at all** (found
     converting `ui/sequencer.lisp`; this is stage 3's documented "read-side
     rescue, not two-way aliasing" caveat firing forward for the first time,
     and it is the strongest remaining argument for finishing the migration).

     The bare reference interns `<module>/<name>`. The late-binding heal
     aliases that slot to the owner's cell on first read — correct at that
     instant. The owner's next `(set! …)` is a `StoreGlobal` that replaces the
     owner's slot `Option` and unlinks the alias, and the module's slot keeps
     the *old* cell forever. `cursor-step` hit exactly this: `eseq.sequencer/
     cursor-step` read 0 while flat `cursor-step` held 8, so selecting a track
     stopped repainting its cursor. Two tests failed; nothing errored.

     The mirror case is worse because it is silent in both directions: a
     module's bare `(set! step-click-pending nil)` interns and writes the
     module's own slot, so the vanilla owner never sees the reset. (Hazard (j)
     is the special case of this where the owner does not exist yet; this is
     the case where it does.)

     Exposure is precisely **plain `def` + somebody `set!`s it**:

     - **`(def x (state …))` is a `defstate` in disguise — alias it, do not
       pin it** (found converting `ui/patch-macros.lisp`, S3b wave 10). The
       `(state …)` initializer routes the form through
       `compile_named_state_definition`, the *same* path `defstate` takes
       (compiler.rs:1787-1798), so the name lives in the `state_bindings`
       keyspace and inherits the immunity below — a flat `set!` from a test
       reaches the qualified binding through `state_binding_for`'s compat-alias
       rung (compiler.rs:1432-1436) as a `StoreState` on the identical node.
       `patch-macros-filter` was pinned on the first pass and reverted. When
       classifying a file's globals, read each `def`'s *initializer*, not just
       its keyword: only a genuinely plain value makes it a pin candidate.
     - `defstate` is immune — it resolves through `state_bindings` on the flat
       key at compile time and never touches the global ladder. This is why
       `selected-bus`, `lower-panel-buffer` and `drum-step-cursor-*` were fine
       in the same file, and why `ui/mixer.lisp` got away with reading and
       writing `selected-bus` bare.
     - write-once globals (`page-size`, `page-button-width`) are harmless: the
       heal never gets unlinked.

     Requalifying the reference does **not** fix it — every spelling compiles
     to one fixed global index, and `eseq.vanilla/<name>` interns its own slot
     the same way. The fix is to go through a **function** the owner supplies,
     because function slots are written once by their `def` and so the heal
     survives: `cursor-step-value` in `ui/seq-core-state.lisp` and
     `step-clear-drag-state` in `ui/step-grid-interactions.lisp`. That is the
     right ownership boundary anyway — a module reaching into another file's
     mutable variable was always the smell — but it means **converting a file
     can require adding accessors to the vanilla files it depends on**, which
     no earlier batch needed. Per-file check: list the converted file's bare
     outbound references, keep only the ones whose owner declares them with
     `def` (not `defstate`), and drop the ones nothing ever `set!`s. What is
     left needs an accessor.

  n. **A file whose source Rust tests eval in SLICES cannot use `import`
     aliases — spell the module out in full** (found converting
     `ui/step-grid-interactions.lisp`, S3b wave 8).

     `state_values::tests` drives several gesture harnesses by reading a UI
     lisp file, cutting a *substring* of it (`load_step_gesture_source`,
     `load_keyboard_step_selection_source` slice from one `(def …)` to
     another), and eval'ing that fragment alone. The `(import … :as core)`
     line sits above the cut, so it is not in scope in the fragment — and the
     two spellings fail asymmetrically at `Compiler` (compiler.rs:1273-1288):

     - `core/cool-off-follow` — the namespace is **undotted**, which the
       compiler reads as alias-shaped and therefore a typo'd or missing
       import: a hard `errors.push`. Twelve tests broke.
     - `eseq.seq-core-state/cool-off-follow` — **dotted**, so it is a full
       module name, which only `warn_once`s (the §3 escape hatch has to be
       able to name a module before it loads) and then heals onto the
       harness's flat natives.

     So the conversion uses the full dotted spelling for every cross-module
     reference, and says why in its header. The rule generalizes: **`:as`
     aliases are a whole-file convenience and are only safe when the whole
     file is always compiled as a unit.** Grep a converting file's path in
     `crates/sequencer/src` for `read_to_string` + `find(`/`[..]` slicing
     before choosing the spelling. This is the third distinct way the Rust
     test harness constrains a conversion, after the hazard-(a) key
     assertions and the hazard-(m) native re-`def`s — the pattern is that
     *how Rust loads the lisp* is as much a part of a file's contract as
     what the lisp says.

     **(n2) A file that any Rust harness evals STANDALONE cannot `import` at
     all** (found converting `ui/piano-roll.lisp`, same wave).
     `metal_seq_piano_roll_lisp_loads` and
     `sync_piano_roll_state_applies_pending_track_fit_after_items_update`
     `read_to_string` the *whole* file — no slicing, so (n) proper does not
     fire — but they eval it into a bare `Runtime::new()` whose source
     manager has **no `@/` root**. `import` resolves its target through
     `module_file_candidates`, which would fall through to a cwd-relative
     `seq-core-state.lisp` that does not exist, pushing a load error into
     every such VM. So piano-roll adds no imports at all and reaches
     `cool-off-follow` bare through eseq.seq-core-state's identity alias.
     Check for standalone evals with the same grep as (n); the two failure
     modes are distinguished by whether the harness slices, and the safe
     answers differ (dotted spelling for (n), *no import* for (n2)).

     **(n3) Do not quote the slice boundary literals in your own header
     comment.** The slicer is a plain `str::find` on two string literals, and
     it takes the FIRST match. A conversion whose new header comment quotes
     the boundary `(def …)` forms verbatim moves the cut into the comment and
     the fragment evals as garbage (`ParseError`). `seq-panels.lisp` hit this
     on its first draft; its header now *describes* its two boundaries
     instead of spelling them, and says why. This is a booby trap unique to
     conversions, because conversions are exactly when a file grows a large
     new header.

  o. **A cross-file call that runs at RENDER time, against a file that loads
     LATER, caches empty forever once the caller becomes a module** (found
     converting `ui/transport.lisp`, S3b wave 10 — the hardest failure of the
     batch, and the one with the most consequence for slice 4).

     `ui/transport.lisp` loads at `main.lisp:25`; `seq-arrangement-view?`'s
     owner `ui/seq-step-tabs.lisp` loads at `:41`. Transport's two view-button
     subtrees call it **during render**, and the effect body runs once at
     load — i.e. while the owner does not yet exist.

     Headerless, this survives by accident: the bare reference interns the
     *flat* slot, and the later vanilla `def` fills that very slot. As a
     module the reference interns `eseq.transport/seq-arrangement-view?`, the
     late-binding heal has nothing to land on at that instant
     (`unknown_global`), and — the part that makes it permanent — **no
     reactive dependency is recorded**, because the failing `LoadGlobal`
     errors out before `record_symbol_read`. The two subtree owners therefore
     cache their empty result and nothing ever re-runs them.

     Two non-fixes worth knowing, both tried:
     - **Pinning does not help.** `eseq.vanilla/seq-arrangement-view?` interns
       its own slot and heals through the same still-empty flat slot.
     - **An alias does not help either**, for the same reason: the owner has
       not run, so there is nothing to alias *to* yet.

     The fix is **`import`**, and it is the one hazard where import is
     load-bearing rather than stylistic: `import` evaluates its target (§4),
     so the owner is guaranteed to exist before the caller's render runs.
     `eseq.transport` therefore carries `(import eseq.seq-step-tabs :as tabs)`.

     Per-file check: **list every cross-file call that executes at render
     time — inside a widget/subtree body, not inside an `on-click` lambda —
     and check it against `main.lisp` load order.** Event-time calls are
     always safe (everything has loaded by the time a key or click arrives);
     load-time and render-time calls to a later-loading file are not.

     **This is the standing hazard for slice 4 (`eseq-mods.9`).** Dissolving
     the `main.lisp` manifest *changes load order by construction*, so every
     edge of this shape becomes live at once. The manifest's ordering comments
     are load-bearing for exactly this reason, and the safe dissolution order
     is: add the `import` that encodes each ordering constraint FIRST, verify,
     and only then delete the corresponding `(load …)` line.

  p. **RESOLVED (eseq-mods.12): `import` now has a compile-time half — see
     §4.** The hazard as found: `import` was a RUNTIME form and could not
     supply anything the importing file needs at COMPILE time (found
     dissolving the manifest, `eseq-mods.9` — the hazard that decided how
     far slice 4 could go). The mechanism that resolved it: the compile
     unit is split at top-level `(import …)` forms and compiled/executed
     segment by segment, so each import's target is evaluated before any
     later form compiles and the continuation compiler re-seeds its
     `state_bindings`/macro/alias tables from the VM. The "callback into
     the VM mid-compile" shape the original proposal sketched is not
     structurally possible — `eval_str` MOVES the VM's chunk and
     global-name tables into the compiler for the duration of a compile —
     which is why the split happens at the driver level instead. The
     original writeup follows for the record; its closing rule
     ("compile-time dependencies are ordered by the LOADER") is retired.

     A file is compiled *in full* before any of it executes, so an `(import
     eseq.x)` inside file F runs after F is already compiled. It therefore
     guarantees exactly one thing: **eseq.x is evaluated before F's body
     runs.** That is what makes it the fix for hazard (o) — render/load-time
     *calls* resolve through the runtime heal ladder, and by then the target
     exists.

     It is *not* enough for anything the compiler resolves while compiling F:

     - **`defstate` reads.** `Compiler::state_binding_for` looks the name up
       in a `state_bindings` table seeded from the VM when F's compiler is
       built. If the owner has not been evaluated by then, the read compiles
       as an ordinary `LoadGlobal` instead of a state read, and a later
       vanilla `(set! name v)` — which *does* see the binding — writes
       somewhere the reader never looks. `:refer` does not rescue this: it is
       consulted *by* `state_binding_for`, so it misses for the same reason.
     - **Macros** (§10 hazard h) and **compat-alias spellings**, for the same
       "seeded at compiler construction" reason.

     Reproducer: adding `(import eseq.seq-core-state)` to `ui/browser.lisp`
     broke `metal_seq_browser_audio_effect_activation_uses_selected_bus` even
     though `ui/main.lisp` had already evaluated that module — the harness
     evals browser.lisp into a bare `Runtime`, so the import was browser's
     *only* source of the module and arrived one phase too late. Removing the
     import fixed it; the module's evaluation was already ordered by the
     loader, which is the only thing that can order compile-time surface.

     ~~Rule: compile-time dependencies are ordered by the LOADER, runtime
     dependencies by `import`.~~ **Retired by eseq-mods.12**: an import now
     supplies compile-time surface too, so a module that consumes another
     module's `defstate`s or macros simply imports it. The reproducer above
     inverts — with the compile-time half, `(import eseq.seq-core-state)`
     in `browser.lisp` makes the standalone-harness eval WORK (the import
     evaluates the real module before browser's readers compile), and the
     test above passes with the import present.

  **(n2) corrected.** The claim that a file evaled standalone "cannot
  `import` at all" is too strong. `module_file_candidates` resolves against
  the source manager's cwd, which for the `metal_seq`/`state_values`
  harnesses *is* `crates/sequencer`, so `ui/<name>.lisp` resolves fine and
  the import evaluates. The real hazard is semantic, not resolution:
  importing pulls a **real** module into a world the harness had faked (see
  (p)'s reproducer), and it does so one phase too late to be useful. Treat
  (n2) as "do not add imports to standalone-evaled files *because they buy
  nothing and can change what the harness is testing*", not as "the path
  will not resolve".

  ### Slice 4 outcome — dissolving the `ui/main.lisp` manifest

  `ui/main.lisp` went from **26 `(load …)` lines** to **3 loads + 15
  imports**, and from 64 to 63 lines (the manifest shrank; a header
  explaining the distro-root contract replaced the ordering comments).

  - **Dissolved entirely (8)** — now reached only through declared `import`
    edges: `track-collapse` (imported by browser, mixer, sequencer,
    arrangement), `seq-step-tabs` (transport), `seq-layout`
    (seq-macro-mapping-hooks), `seq-panels` (sequencer),
    `step-grid-interactions` + `seqv-track-params` + `seq-grid-mode`
    (bus-grid), `sound-palette` (arrangement).
  - **`load` → `import`, still listed (15)**: the render roots, which nothing
    imports because their top level *is* the side effect, plus the two
    compile-time hubs pinned to the top by hazard (p) — `eseq.materials`
    (macros, also consumed by the ~305 headerless content files, which
    cannot import) and `eseq.seq-core-state` (the `defstate` hub).
  - **Still `load` (3)**: `themes.lisp` (re-evaluation *is* applying the
    theme), `effects.lisp` (its own nested manifest, out of scope here) and
    `effects/step-buffer.lisp` (headerless side-effect root).
  - **Comments deleted**: the track-collapse ordering comment (replaced by
    four real import edges) and the choose-model one, which was stale — the
    patcher buffers that mount `choose-model-panel` are Rust-generated lisp
    (`edit_sessions.rs`) evaluated when a patch editor opens, so any boot
    position satisfies it. The `step-grid.lisp`-is-deliberately-unloaded
    comment stays: it documents an absence and a perf regression, not order.

  **Order independence: ACHIEVED (eseq-mods.12), with one scoped rule.**
  At slice-4 close, reversing the root block failed 157 `metal_seq` tests
  (hazard (p) plus undeclared root-to-root edges). With import's
  compile-time half (§4) and the follow-up unpinning, the entire import
  block of `ui/main.lisp` — render roots included — now boots in any
  order: the test `metal_seq_main_import_block_boots_in_reverse_order`
  boots a fully reversed block and asserts the same buffers render and the
  formerly order-pinned cross-module reads agree. What made it true:

  1. Hazard (p) resolved: consumers of `eseq.seq-core-state` and
     `eseq.seq-step-tabs` `defstate`s/aliases now import them
     (seq-script-picker → seq-step-tabs, browser/mixer/sequencer/transport/
     seq-panels/seq-layout/seq-macro-mapping-hooks → seq-core-state), so
     `eseq.materials` and `eseq.seq-core-state` are listed but no longer
     order-pinned root entries.
  2. The one load-time value edge INTO a render root was inverted:
     `piano-roll-default-pane-height` moved home from `eseq.piano-roll` to
     the layout hub `eseq.seq-step-tabs`, and piano-roll + seq-layout
     import the hub. The **never-import-a-UI-root rule stands** (the four
     roots plus transport/agent/patch-macros/piano-roll/effects.buffers):
     order freedom is achieved not by declaring root→root edges but by
     ensuring no module needs one at compile or load time — roots may
     import library modules, never each other. Remaining root-to-root bare
     references are event-time or import-covered render-time calls.

  Scope note: `load` lines stay ordered relative to the code that uses
  them (themes.lisp before the theme call); reorderability is a property
  of the `(import …)` block.

  **Hot reload (§11 q4) — answered, and it needed a fix.**
  `reload_paths_transactional` re-evaluates a changed file's *owner root*,
  which after this slice reaches its children through `import`. Load-once
  would therefore make that re-eval skip every child and silently drop the
  edit — the whole `ui/` tree would stop hot-reloading. `import` is now
  load-once **per import pass**: `VM::imported_at_epoch` + `import_pass_epoch`,
  bumped by `begin_import_pass()` from both transactional eval entry points.
  `(module …)` records the epoch too, so a `load`ed module is still not
  double-evaluated by a later `import` in the same pass. Covered by
  `hot_reload_reevaluates_imported_children_from_the_owner_root`, which fails
  against the old permanent ledger. Import edges themselves were already
  recorded in the `ModuleGraph` exactly like `load` edges (`__import-module`
  goes through `SourceManager::load_source`), so the graph shape is unchanged
  — only now it is declared rather than inferred.

  **Step 4 — validate.** `cargo build -p eseqlisp -p sequencer`,
  `cargo test -p eseqlisp`, `cargo test -p sequencer`, plus the specific
  tests that load the converted file's family. Consumers stay untouched;
  the only edits outside the converted file should be requalified test
  expectations (hazard a).

  **Blocker found while validating batch 1 — FIXED.** The whole "load
  `ui/main.lisp`" test family (~170 tests in the `metal_seq` bin) was
  failing with `ui/piano-roll.lisp: eval error: IncorrectType`, bisected to
  **S0** (`1f02c2e8`, implicit-`eseq.vanilla` interning) and invisible
  because those tests overflow the default 8 MB test stack and abort the
  binary before reporting. `cff706ec` fixed the three resolution-ladder gaps
  behind it (module def-sites always intern qualified, runtime late-binding
  heal for empty qualified slots, reactive-namespace exemption). Standing
  rule from it: **run `cargo nextest run -p sequencer -E
  'binary(metal_seq)'` per conversion**. Since eseq-4tl,
  `.cargo/config.toml` supplies the documented 16 MiB test-stack budget
  automatically, so this gate reports failures by test name instead of relying
  on a remembered `RUST_MIN_STACK` prefix.

  **Step 0 addendum (batch 2, superseded for globals by stage 3).** The
  load-order gate applied to Rust test harnesses that evaluate a consumer's
  *source* directly (`eval_str(&read_to_string("ui/mixer.lisp"))`), not just
  to production manifests: the consumer's own top-of-file
  `(load "@/ui/dep.lisp")` is as late there as it is in production, so each
  such harness needed a separate, earlier eval of the dep. Six of them
  needed one for `track-collapse`. Those edits stay, but with the stage-3
  heal a def-only conversion no longer requires them — only a harness whose
  consumer expands the converted file's *macros* still does.

  **Batch 3 tally (2026-08-12).** Infra: the stage-3 heal above. Converted:
  `ui/sound-palette.lisp` → `eseq.sound-palette` (23 defs, 12 `%`-private, 3
  aliases, hazard a — 9 requalified key assertions),
  `ui/effects/state.lisp` → `eseq.effects.state` (32 defs, 23 identity
  aliases, no renames — six unrelated prefix families in one hub file, so
  stripping collides; hazard i found and 7 names kept in vanilla),
  `ui/seq-layout.lisp` → `eseq.seq-layout` (25 defs, 14 `%`-private, 8
  aliases; first module→module reference in the migration, resolved through
  the alias rung). Vetoed: `ui/seq-grid-mode.lisp` on hazard d.

  Running conversion count: 9 files. What batch 3 changes for the big four
  (`sequencer`, `browser`, `arrangement`, `mixer`): load order is no longer
  a gate for their globals, so they can be attacked in any order; the two
  live gates are hazard (a) — those files own most of the app's widget
  `:key`s and therefore most of the ~280
  `find_layout_node_by_stable_key` assertions — and hazard (d), which vetoes
  any of them that defines a mode until the mode keyspace gets its alias
  rung.

  ### Batch 4 — infrastructure stage (BUILT 2026-08-12)

  Two pieces of infra, no conversions:

  1. **Mode keyspace alias rung** — hazard (d) stage 4 above. This lifts the
     batch-3 veto: `ui/mixer.lisp` defines `seq-mixer-mode` and is now
     convertible, and `ui/seq-grid-mode.lisp` is unblocked as a separate
     candidate.
  2. **`find_layout_node_by_stable_key_suffix` in
     `crates/sequencer/src/ui/state_values/tests.rs`** — ported from
     `ui/tests.rs`, byte-identical semantics. Hazard (a) re-keys every widget
     in a converted file; a conversion rewrites its own assertions as
     `_suffix(&layout, "/entry-0")`, which pins the key without naming the
     owning module. Deliberately duplicated rather than hoisted: that is the
     existing pattern (`find_layout_node_by_stable_key` already lives in both
     files), both `mod tests` blocks are private, and the crate has no
     test-support module. Only the 11 already-requalified choose-model /
     sound-palette sites were migrated, as worked examples — a bulk rewrite
     would bury each conversion's own diff.

  ### Batch 4 preflight table

  Derived by sweeping `crates/sequencer/src` (and `crates/eseqlisp/src`) with
  whole-file — **not line-based** — regexes for every one of the 591 names the
  four files define. Line-based grep misses the multi-line raw-string lisp in
  `ui/host_commands/scripts.rs` and `ui/input.rs`; that is how the first pass
  under-reported `sbrowser-tab`. Patterns: writes `(set!|def|defstate|defwidget) <name>`
  and `set_global_value("<name>")`; reads `global_value`/`invoke_global`/`has_global`;
  calls `(<name>` inside Rust string literals.

  **Column 1 — pinned to `eseq.vanilla` via the §3 escape hatch, no alias.**
  Production Rust writes the name by bare spelling. Per hazard (i)'s rule of
  thumb these are a host→script protocol, not the module's API. Write them
  `(def eseq.vanilla/<name> …)` / `(defstate eseq.vanilla/<name> …)` inside
  the module and mint **no** `module-compat-alias` for them.

  | file | pinned names | production writers |
  |---|---|---|
  | `browser.lisp` | `sbrowser-tab` (defstate) | `host_commands/scripts.rs:115,232,260`; `host_commands/tracks.rs:335` |
  | | `sbrowser-editor-name` | `host_commands/instrument_authoring.rs:158,369,2317,2533,3339,3486` |
  | | `sbrowser-loading-instrument-name` (defstate) | `ui/event_loop.rs:1387`; `host_commands/tracks.rs:506,517,527,538,553,561,575,591` |
  | | `sbrowser-script-name` (defstate) | `host_commands/scripts.rs:114,231,259` |
  | | `sbrowser-script-save-mode` (defstate) | `host_commands/scripts.rs:113,230,258` |
  | | `sbrowser-auditioned-sample` (defstate) | `ui/edit_sessions.rs:1091` |
  | `sequencer.lisp` | — none — | |
  | `arrangement.lisp` | — none — | |
  | `mixer.lisp` | — none — | |

  All six pinned names are in `browser.lisp`, and all six are also written
  from `ui/state_values/tests.rs`. **`browser.lisp` is the only one of the
  four carrying hazard (i) at all.**

  **Column 2 — alias-safe, but a test fixture *defines* the name.** These are
  `#[cfg(test)]` stubs (everything in `ui/input.rs` from line 1687 —
  `mod live_keyboard_tests` — and the lisp preludes in
  `ui/state_values/tests.rs`). They are alias-safe in principle: writes follow
  the alias, and a redefinition keeps last-writer-wins exactly as it does
  today against a flat name. But they are the *stub-shadows-the-real-thing*
  pattern, and stage 2's macro finding — "an alias should not be minted for a
  name that unconverted files redefine on purpose" — applies in spirit.
  **Re-run the owning tests after minting each of these aliases.**

  | file | stub-defined names |
  |---|---|
  | `sequencer.lisp` | `seqv-collapse-all-tracks`, `seqv-current-number-picker-key`, `seqv-current-param-mode`, `seqv-current-selected-step`, `seqv-expanded-track-ids` (defstate), `seqv-select-all-current-track-steps`, `seqv-select-track-for-edit` — all `ui/input.rs` 2296–3682 |
  | `browser.lisp` | `sample-browser-here`, `sbrowser-active-tree-key`, `sbrowser-next-tab`, `sbrowser-filter` (`ui/input.rs` 1925–1945); `sbrowser-add-selected-rack-layer`, `sbrowser-sample-selected-path`, `sbrowser-preset-filter`, `sbrowser-selected-instrument-name`, `sbrowser-selected-audio-effect-name`, `sbrowser-selected-tags` (`state_values/tests.rs`) |
  | `arrangement.lisp` | `arrangement-cursor-time`, `arrangement-cursor-track`, `arrangement-view-start`, `arrangement-view-duration` — all `state_values/tests.rs` + `ui/tests.rs:9701`, `set!` only (no stub `def`), so plain alias-covered |
  | `mixer.lisp` | — none — |

  **Column 3 — alias-safe reads/calls.** Rust names these globals only to read
  or invoke them; every such path goes through `resolve_global_read_index`,
  which has the alias rung. Mint a normal alias and move on.

  | file | production read/call | test-only read/call |
  |---|---|---|
  | `sequencer.lisp` | `seqv-select-track-for-edit` (`ui/input.rs:153` `global_value`+`invoke`, with an `eval_str` fallback at `:160`); `seqv-collapse-all-tracks`, `seqv-current-param-mode`, `seqv-current-selected-step`, `seqv-current-number-picker-key`, `seqv-select-all-current-track-steps` (`ui/input.rs` 476–1277, inside the `#[cfg(test)] pub(crate) fn handle_metal_command_shortcut` at :1033 — real dispatch logic, test-gated) | 16 `seqv-*` entry points driven from `state_values/tests.rs` / `ui/tests.rs` |
  | `browser.lisp` | `sbrowser-refresh-buffer` (`ui/edit_sessions.rs:1100`, `host_commands/instrument_authoring.rs:2346,2558`, `host_commands/project.rs:50`); `sample-browser-here` (`ui/input.rs:228` `global_value`+`invoke`, fallback `(switch-to-buffer "*samples*")`); `sbrowser-active-tree-key` (`ui/input.rs:245`) | 29 `sbrowser-*` entry points |
  | `arrangement.lisp` | — none — | 24, incl. `arrangement-track-clips`, `arrangement-lane-selection`, `arrangement-scene-action`, `set-arrangement-view-start` |
  | `mixer.lisp` | `seq-ctrl-g` (`ui/input.rs:1307`, test-gated dispatch) | 13 `mixer-v2-*` entry points |

  **Column 4 — lisp-side external callers (the aliases you must mint).**
  Fan-out is small; this is the complete list.

  | file | names referenced from other lisp files |
  |---|---|
  | `sequencer.lisp` (196 defs) | `sequencer-cursor-step-changed`, `seqv-collapse-all-tracks`, `seqv-handle-key`, `seqv-open-piano-roll-for-track`, `seqv-select-track-for-edit`, `seqv-set-param-mode`, `seqv-toggle-current-track-expanded`, `seqv-track-header`, `seqv-track-menu-click`, `seqv-track-selected-binding` — plus `metal-track-tick`, which is a **`defwidget`** and therefore needs **no** alias (hazard e: widget names do not qualify) |
  | `browser.lisp` (147) | `sbrowser-drop-instrument-on-track`, `sbrowser-drop-sample-on-track`, `sbrowser-drop-sound-on-track`, `sbrowser-enter-preset-save`, `sbrowser-open-project-save`, `sbrowser-project-save-mode?`, `sbrowser-tab` (pinned — no alias), `sbrowser-loading-instrument-name` (pinned — no alias) |
  | `arrangement.lisp` (128) | `set-arrangement-cursor`, `arrangement-ghost`, `arrangement-view-start`, `arrangement-view-duration` (three of the four are reached only from `ui/capture-fixtures/*.lisp`) |
  | `mixer.lisp` (120) | `mixer-v2-muted?`, `mixer-v2-track-collapsed-label`, `mixer-v2-track-color-r/-g/-b`, `patch-mixer-strip`, `track-peak` |

  **Not covered by this table, and out of scope for the four:** the dynamic
  bare-name globals in `lisp_host/native_arg_parsing.rs:52/91/109` (per-effect
  param descriptors, `sanitize_symbol_name`-derived, and nil'd out by bare
  name) and the `__scratch_hook_{N}` family (`app/effects.rs:270`). None
  collide with a name any of the four defines, but they are the reason a
  module system needs a permanent escape hatch, not just a migration one.

  ### Batch 4 per-file risk and recommended order

  1. **`mixer.lisp`** — lowest risk, convert first. **Zero** Rust writers,
     zero pinned names, 7 lisp-side aliases, ~10 stable-key assertion sites.
     Its `define-mode "seq-mixer-mode"` is fully self-contained (it defines
     the mode, binds only handlers it defines itself, and calls
     `set-buffer-mode-for "*mixer*"` itself), so every mode reference
     qualifies consistently and it needs **no mode alias at all**. It is the
     acceptance test for the stage-4 rung with the least else going on.
  2. **`arrangement.lisp`** — no Rust writers, no modes, no `:shader` /
     `:material`, 4 lisp aliases (3 reached only from capture fixtures),
     ~12 assertion sites. The only wrinkle is `set!`-only test writers, which
     the alias covers.
  3. **`sequencer.lisp`** — hazard (a) is concentrated here: ~95 of the ~235
     stable-key assertion sites come from its 39 dynamic key prefixes
     (`seqv-*`, `sequencer-track-*`). Also the only one with hazard (h): its
     single `defmacro seqv-aqua-slider-track-material` is used from two
     `:material` bodies **in the same file**, which expand in the throwaway
     implicit-module compiler — so those two call sites must be rewritten
     **qualified** (`eseq.sequencer/seqv-aqua-slider-track-material`) or they
     will not resolve. No external macro caller, so no step-0 gate. It
     references `seq-grid-mode` (`sequencer.lisp:1957`) but does not define
     it: that reference qualifies to `eseq.sequencer/seq-grid-mode` and lands
     on vanilla through rung 3, pinned by
     `module_mode_reference_falls_back_to_a_vanilla_mode`. It also carries the
     live hazard (j) (`:381`) and, being the biggest of the four, the most
     hazard-(k) surface: run the binder intersection before trusting a green
     suite.
  4. **`browser.lisp`** — highest risk, convert last. It is the **only** one
     of the four with hazard (i), and it has six pinned names — five of them
     `defstate`, so hazard (b) compounds: a pinned `defstate` must keep both
     its global slot and its `state_bindings` key flat, and pinning is the
     §3 escape hatch on the `defstate` form itself. Two of the pinned names
     (`sbrowser-tab`, `sbrowser-loading-instrument-name`) are also referenced
     from other lisp files, which is fine precisely *because* they stay flat.
     44 literal widget keys (the most of the four) but only ~14 assertion
     sites.

  ### Batch 4 tally — `mixer.lisp` (2026-08-12)

  `ui/mixer.lisp` → `eseq.mixer`. 120 defs: 98 `%`-private, 18 renamed public,
  4 `defwidget` names left flat (hazard e). 19 compat aliases — 18 renames plus
  an identity alias for `seq-ctrl-g`, whose spelling is unchanged but which
  `src/ui/input.rs:1307` evals by name.

  **The mode-alias rung (stage 4) is accepted, unexercised.** `seq-mixer-mode`
  is genuinely self-contained — it appears nowhere outside `mixer.lisp` in lisp,
  Rust, or a `(current-buffer-mode)` comparison — so it qualifies consistently
  on both sides and needed **no** alias. The three handler strings
  (`:on-key`, two `mode-bind-key`) follow the renames and dispatch through the
  module. `seq-grid-mode.lisp` remains the file that will actually exercise the
  flat→qualified rung.

  **Preflight-table corrections.** Column 4 over-counted mixer's lisp-side
  callers at 7; the real number is 5. `patch-mixer-strip`'s only mention outside
  the file is a *comment* in `seq-layout.lisp`, and `track-peak`'s only other
  definition is `ui/legacy/mixer.lisp`, which nothing loads — it is now
  `%track-peak` and deliberately unaliased, per stage 2's rule about names other
  files define on purpose. Both corrections came from grepping the names rather
  than trusting the table, which is the standing instruction. In the other
  direction the table's "~10 stable-key assertion sites" is badly low: mixer has
  **37**, 26 on auto-qualifying widget keys and 11 on non-qualifying subtree
  keys (see hazard (e) above for why the two halves are handled oppositely).
  Assume the same undercount for the remaining three files — the number to
  budget for is the count of *all* `:key` sites, split by keyspace, not the
  count of `find_layout_node_by_stable_key` calls that happen to mention the
  file's prefix.

  Hazard (j) is new and was found here. Hazards (b), (c), (h) and (i) had no
  exposure in this file, as predicted.

  Running conversion count: 10 files.

  ### Batch 4 tally — `arrangement.lisp` (2026-08-12)

  `ui/arrangement.lisp` → `eseq.arrangement`. 128 defs, all renamed by dropping
  the `arrangement-` prefix: 93 `%`-private, 35 public. 35 compat aliases, one
  per public name. Nothing is left flat — the file has no `defwidget`, no
  `defchan`, no `defhook`, no `defmacro`, no `define-mode`, no
  `:shader`/`:material`, and no production Rust writer, so hazards (c), (d),
  (h) and (i) have zero exposure exactly as the preflight table predicted.

  **Hazard (j) has zero exposure, and the reason generalizes.** All 11 outbound
  `set!`s target the file's own defs, and — unlike `mixer.lisp` — nothing evals
  arrangement's source standalone: `main.lisp:59` is the only loader, and
  `state_values/tests.rs:849` merely *parses* it in the syntax-lint test. The
  check to run per file is therefore two-part: (1) do any outbound `set!`s
  exist, and (2) is there a harness that evals this file without its peers. A
  "no" to either retires the hazard for that file.

  **Preflight-table corrections.** Column 4's four lisp-side names are exactly
  right (`set-arrangement-cursor` from `transport.lisp:456`; `arrangement-ghost`
  / `-view-start` / `-view-duration` from `ui/capture-fixtures/*.lisp` only).
  What it misses, as with mixer, is that lisp callers are not the alias set:
  adding the Rust test files that eval names takes it from 4 to **35**. Three
  names the sweep surfaces get **no** alias: `arrangement-region-all-tracks` and
  `arrangement-windowed-dots` appear only in Rust *comments* (the mixer
  `patch-mixer-strip` precedent), and every hit for `arrangement-scene-lane`
  outside the file is the widget `:key` string of the same spelling rather than
  a call — the def is now `%scene-lane` and the string is handled as a key.
  Column 2's four `set!`-only stub writers are confirmed alias-covered through
  the hazard-(b) `defstate` ladder; no test file `def`s any of them.

  Column 3 / hazard (a) undercount again: **21** assertion sites outside the
  file against the table's "~12", from just **6** widget `:key` sites inside it.
  20 lookups plus one `format!`-built key move to the `/`-suffix matcher,
  including three shared `lane`/`lane_rect` closures rewritten in their bodies.
  The **5** subtree `:key`s and the 2 assertions on `arr-track-0` stay
  byte-identical, per the keyspace split. So do the `"arr-*"` SEQV channel
  strings that `(channel name i)` builds — a third keyspace the tests read by
  exact spelling.

  Hazard (k) is new and was found here: exactly one of the 128 shortened names
  collided with a pre-existing local binding, silently changing what the global
  reference resolved to, with the whole test family still green. The binder
  intersection described in (k) is now a required conversion step.

  Running conversion count: 11 files.

  ### Batch 4 tally — `sequencer.lisp` (2026-08-12)

  `ui/sequencer.lisp` → `eseq.sequencer`, the largest file of the four. 196
  defs: 10 `defwidget` names left flat (hazard e — four are also `:background`
  string values Rust asserts on), 155 `%`-private, 29 public, and
  `sequencer-cursor-step-changed` pinned to `eseq.vanilla`. **27 compat
  aliases** against the preflight's 10 lisp-side names; the nine non-widget
  entries in column 4 are all correct, and `metal-track-tick` is correctly
  flagged as a `defwidget` needing none. Four names appear outside the file and
  still get **no** alias: `seqv-track-color-r` (a Rust comment),
  `seqv-track-volume-control` and `seqv-playhead-row` (widget `:key` strings
  rather than calls — the arrangement `scene-lane` precedent), and
  `seqv-step-cell`, whose only other mention is `editor/tests.rs` defining its
  own in a standalone harness (the mixer `track-peak` precedent).

  **The stub-then-override is resolved by pinning, not by aliasing.**
  `step-grid-interactions.lisp` defines a nil `sequencer-cursor-step-changed`
  and calls it from `set-track-cursor-step`; sequencer.lisp's later def is what
  moves the cursor. No alias can reach that caller — it compiled at
  `main.lisp:46`, before the aliases existed at `:57`, and the heal only
  repairs an *empty* slot, which the stub had already filled. So the flat name
  keeps its flat spelling through the §3 escape hatch
  (`(def eseq.vanilla/sequencer-cursor-step-changed …)`) and forwards into
  `eseq.sequencer/cursor-step-changed`. **Generalizes: an alias rescues a
  caller of a name nobody defined; a stub-then-override pair needs the flat def
  to stay flat.** This pair is the S4 `defhook` candidate.

  **The mode ladder needs nothing.** The file references `seq-grid-mode`
  without defining it, and that reference lands on vanilla through stage-4
  rung 3 as predicted. The reverse edge — vanilla's
  `(mode-bind-key "seq-grid-mode" "C-h" "seqv-collapse-all-tracks")` naming a
  handler that now lives in a module — also needs no mode alias, because
  handler dispatch runs `invoke_global` → `resolve_global_read_index`, which
  already has the alias rung. Worth recording for `seq-grid-mode.lisp`'s own
  conversion: the handler keyspace is covered by the ordinary global alias.

  **Hazard (h), the only exposure in the big four**, went as the preflight
  said, plus a trap it did not anticipate: the macro could not simply be
  stripped to `aqua-slider-track-material`, because that is `ui/materials.lisp`'s
  compat alias for `eseq.materials/slider-track-material` — and in the very
  implicit-module expansion that forces the call sites to be qualified, the
  alias rung would have won and expanded the wrong macro. Renamed
  `step-slider-track-material`. **A converted file's macro renames must be
  checked against the compat-alias table, not just the macro table.**

  **Hazard (j) is live, as the mixer tally predicted for `:381`.** Two
  harnesses eval this file without `ui/browser.lisp`, and both now declare
  `sbrowser-loading-instrument-name`.

  **Hazard (a): 40 `:key` sites, three keyspaces** — 37 widget keys that
  qualify and drop their prefix, 3 subtree keys byte-identical, and the SEQV
  channel strings untouched. 100 assertion rewrites outside the file, against
  the preflight's "~95": the count that mattered was the `:key` split, not the
  lookup count. Two loops needed a mixed treatment, keeping the subtree key
  exact next to `/`-suffixes.

  **Three new hazards, all found here.** (l) — a lisp helper that hands a
  widget key *out* to Rust must emit the qualified spelling. (m) — a module's
  bare reference to a mutable vanilla `def` global freezes on first read, and
  its bare write never reaches the owner; fixed with owner-side accessors,
  which is the first time a conversion had to modify the vanilla files it
  depends on. And hazard (k) is widened: the strip must be swept against every
  global in the app, which turned up 11 collisions here against arrangement's
  1 — including 7 wrapper/delegate pairs that would have become unbounded
  recursion, and `param-mode`, a vanilla `defstate` whose collision made every
  call to the module's own `param-mode` fail with `ExpectedFunction`.

  Running conversion count: 12 files. **`browser.lisp` is the last of the
  four**, and it now inherits three checks the earlier three did not run: the
  app-wide collision sweep (k), the key-returning-helper grep (l), and the
  mutable-vanilla-global audit (m) — the last is likely to bite, since
  `browser.lisp` is also the only file carrying hazard (i), i.e. it already has
  six names Rust writes by bare spelling.

  ### Batch 4 tally — `browser.lisp` (2026-08-12), and the big four are done

  `ui/browser.lisp` → `eseq.browser`. 147 defs: 4 `defwidget` names left flat
  (hazard e), 6 pinned to `eseq.vanilla`, 46 public, 91 `%`-private; one new
  helper (`%tree-key`) makes 148. **46 compat aliases.** The preflight's column-4
  list of 8 lisp-side names is right, but two of them (`sbrowser-tab`,
  `sbrowser-loading-instrument-name`) are the pinned pair and correctly get none;
  the full sweep takes the set from 6 to 46.

  **Hazard (i) confirmed exactly, and it moved infrastructure.** Re-sweeping
  `crates/*/src` with whole-file regexes reproduces the preflight's six names
  precisely — every other Rust writer is `#[cfg(test)]` (`ui/input.rs` from
  `:1687`, and standalone harness stubs). What the table could not predict is
  that five of the six are reactive state, and **the §3 escape hatch did not
  reach the `defstate` keyspace**. `qualify_registration_name` returned an
  already-qualified name verbatim, so `(defstate eseq.vanilla/sbrowser-tab …)`
  registered `eseq.vanilla/sbrowser-tab` while every flat reader and writer looks
  up `sbrowser-tab`, and neither state-binding ladder has an implicit-module
  rung: the binding is invisible and `(set! …)` StoreGlobals over the slot
  holding the `NodeRef` — hazard (b)'s failure mode, reached through hazard (i).

  **Stage 5 (BUILT 2026-08-12): vanilla's registry keyspace is the flat
  keyspace.** `Compiler::qualify_registration_name` strips an explicit
  `eseq.vanilla/` prefix instead of passing it through, and both
  `Compiler::state_binding_for` and `VM::state_binding_node` reduce the qualified
  escape-hatch spelling to the flat key on lookup. The second half is not
  optional: without it a pinned state *read* written qualified compiles to
  `LoadGlobal` and hands back the raw `NodeRef`. Test:
  `a_vanilla_pinned_defstate_registers_flat` (vm.rs). Generalizes to every
  registration form — a name pinned to vanilla is flat in *all* keyspaces, which
  is what "the implicit module is the pre-module world" has to mean.

  **Hazard (m) is live as a write, and load order does not save it.** The
  sequencer tally's case was a frozen read; here it is
  `(set! seq-script-picker-source-buffer …)`, a mutable vanilla plain `def` owned
  by `ui/seq-script-picker.lisp` — which loads at `main.lisp:42`, *after*
  browser at `:17`. So this is hazards (j) and (m) at once, and neither the heal
  nor load order helps: the write lands in `eseq.browser`'s slot and the owner's
  reader silently keeps returning `""`. Fixed with the owner-side accessor
  `seq-script-remember-source-buffer`. **Second conversion in a row to require
  editing a vanilla file it depends on** — treat accessor-adding as a normal cost
  of conversion, not an exception. The file's only other outbound reference,
  `selected-bus`, is a `defstate` and immune, as in mixer.

  **Hazard (l) is live and was untested.** `active-tree-key` hands a widget key
  to `focus_widget_by_stable_key` (exact match) from `ui/input.rs:245`; it now
  emits `"eseq.browser/<tab>-tab-tree"`. Nothing covered it — the only tests were
  the flat `live_keyboard_tests` stub — so the conversion added
  `metal_seq_browser_active_tree_key_matches_the_rendered_tree_key`, which
  asserts the reported key against the rendered node. **A key-returning helper
  should get a test asserting helper-vs-widget agreement**, because the two
  spellings are maintained in different places and no existing assertion pairs
  them.

  **Hazard (k): 6 collisions in 147 names, and two are new shapes.** Two are
  *natives and builtin widgets*, not lisp globals — `filter` (called three times
  in this very file) and `tabs` — which the sequencer recipe's "every global the
  app owns" does not cover. The practical sweep set is therefore four lists:
  this file's binders, every UI-lisp `def`/alias key, the registered natives, and
  `BUILTIN_WIDGET_NAMES`. The rest: `header` (a `defwidget` in
  `effects/effect-panels.lisp`), `midi-fx-panel` (a vanilla global), plus `tabs`
  also shadowing a `let` head in its own body. Two apparent collisions are
  benign and were kept, which is worth recording because both would waste a
  rename: a **pinned** name never strips, so `sbrowser-tab` cannot collide with
  the `tab` local; and a name owned only by *already-converted* modules
  (`drop-sample-on-track`, in eseq.mixer and eseq.sequencer) has no flat entry to
  merge with. No wrapper/delegate pairs — nothing here is named after the vanilla
  function it calls.

  **Hazard (a) is the mildest of the four despite the most keys.** 45 `:key`
  sites, **all one keyspace**: no subtree keys, no `(channel …)` strings, no
  `:debug-name`s. All 45 qualify; the seven carrying the hand-rolled
  `sbrowser-`/`browser-` prefix drop it. Only **16** external assertion sites
  (against the preflight's "~14" — the first estimate in the batch that was not
  badly low, precisely because there is no second keyspace inflating it), all in
  `state_values/tests.rs`, plus a new
  `collect_layout_nodes_by_stable_key_suffix` (the collecting twin of the batch-4
  finder). The revised budgeting rule: the assertion count tracks the number of
  *distinct* keys Rust names, and the `:key` split predicts how much of the
  rewrite is mechanical. Two production literals in `src/ui/input.rs` try the
  qualified spelling and fall back to the flat one, because the headerless
  `live_keyboard_tests` stub browser reuses the same string; that fallback is
  deleted when the stub qualifies.

  **Zero exposure, as predicted:** (c) no `defwidget :state`; (d) no
  `define-mode`, no `(current-buffer-mode)` comparison; (h) three `:shader`
  bodies but no `defmacro` at all, so nothing of this file's expands outside it.

  Running conversion count: **13 files, and the big four are converted.**

  ### S3b pilot tally — `effects/effect-modulation.lisp` (2026-08-12), import's first production use

  `ui/effects/effect-modulation.lisp` → `eseq.effects.effect-modulation`, the
  first nested module name and the first converted file to reach another
  converted module through `import` instead of the compat-alias rung:
  `(import eseq.effects.state :as st :refer (effect-selected-mod-slot))`
  exercises, in real metal_seq renders, an `:as`-qualified read of another
  module's def (`st/fx-panel-body-content-height`), and a `:refer`red
  `defstate` both read and `set!` (the refer rung of
  `Compiler::state_binding_for`). 14 defs: 13 `%`-private, 1 public rename
  (`effect-mod-control-panel` → `mod-control-panel`), **1 compat alias** — the
  S3b rule is aliases only for unconverted callers (here
  `panel-bodies.lisp`), never for converted→converted edges. The subtree
  `:key` and all `:debug-name` strings stay byte-identical (hazard e; the
  Rust assertions on this file are all debug-names, which do not qualify).

  **Infra fix the pilot forced: import could not resolve `eseq.*` modules
  against the production layout.** `module_file_candidates` produced
  `@/effects/state.lisp` and importing-file-relative spellings, but `@/` is
  the source-manager cwd — `crates/sequencer` in production
  (`enter_sequencer_dir`), whose distro root is the `ui/` subdirectory. So
  the load branch of `__import-module` — unit-tested only with synthetic
  flat layouts — would have failed for every `eseq.*` module the moment a
  manifest stopped pre-loading it (i.e. at eseq-mods.9); load-once was
  masking the gap. Fixed by adding `@/ui/{flat,nested}` candidates (tried
  first), plus `SourceManager::set_cwd` so a test can pin resolution against
  a synthetic root without touching the process cwd. Test:
  `import_resolves_nested_eseq_module_under_the_ui_root` (vm.rs).

  Gate baseline note: the metal_seq gate now reads **586 passed / 2
  pre-existing failures** (`metal_seq_browser_new_instrument_editor_uses_-
  finalize_copy`, `metal_seq_sound_palette_overlay_layout` — both fail
  identically on the pre-conversion tree).

  Running conversion count: 14 files.

  ### S3b wave 1 tally (2026-08-12) — param-controls, drag-drop, panel-widgets, custom-ui-sections, instrument-sources

  Five parallel conversions: `eseq.effects.param-controls` (116 defs, 52
  `%`-private, 0 renames — hub-file precedent, 63 identity aliases for the
  ~28 unconverted callers; imports eseq.macro-state + eseq.effects.state and
  rewrote its two converted callers, effect-modulation and seq-layout, to
  import it), `eseq.effects.drag-drop` (3 defs, 2 renames + aliases),
  `eseq.effects.panel-widgets` (9 defs, 6 renames + aliases, 3 `defwidget`
  names flat), `eseq.effects.custom-ui-sections` (16 defs, 10 identity
  aliases, hazard (i) fired: `custom-ui-selected-section` pinned — written by
  `custom_ui.rs` codegen at `:425`/`:682`; its own read uses the qualified
  escape-hatch spelling), `eseq.effects.instrument-sources` (2 defs, both
  `%`-private, `:refer` onto eseq.effects.state's defstate).

  **New infra finding — hazard (m) applies to natives, and to test stubs of
  them.** `eseq.effects.param-controls` reads `seq-has-selection?` (a Rust
  native) bare; `metal_seq_fx_lisp_lays_out_rack_panel_chain_list` toggles
  that name mid-test with headerless `(def seq-has-selection? () …)` evals.
  Each such def StoreGlobals a fresh cell into the `eseq.vanilla` slot and
  strands the module's healed slot on the previous cell — the module froze
  on a stale value with no error (m's signature). Two-part fix, both
  generalize: (1) **`VM::register_native_with_vm` now mutates an existing
  cell in place** instead of replacing the slot `Option`, so healed alias
  slots track native re-registration (snapshots deep-clone cells, so
  transactional rollback is unaffected); test
  `native_reregistration_reaches_a_healed_module_slot` (vm.rs). (2) Test
  stubs that TOGGLE a native mid-run must use `register_native`, not lisp
  `def` — the toggling evals in `state_values/tests.rs` were converted.
  One-shot prelude `def` stubs that never toggle are unaffected (the module
  slot heals once to the stub's cell). Rule of thumb for later waves: a
  converted module may read a native bare, but if any test rebinds that
  native mid-test by lisp `def`, convert the test to `register_native`.

  Gate: 586 passed / 2 pre-existing. Running conversion count: 19 files.

  ### S3b wave 2 tally (2026-08-12) — filter-core, param-grid, track-panels, process-panel

  `eseq.effects.builtin.filter-core` (first four-segment name; 22 defs, 12
  identity aliases for the 15 unconverted builtin panels, one 15-name
  `:refer` from param-controls, every `:key` a subtree key),
  `eseq.effects.param-grid` (68 defs, 65 `%`-private, 3 identity aliases;
  rewrote its two converted callers to imports),
  `eseq.effects.track-panels` (42 defs, 7 renames + aliases; hazard (k)
  caught `fx-step-param-value` colliding with step-grid-interactions'
  `step-param-value`; ~32 key-assertion rewrites incl. one exact-match
  `focus_widget_by_stable_key` requalified),
  `eseq.effects.process-panel` (24 defs, 5 renames + aliases; `:key`s
  stripped of their prefix, six suffix-matcher rewrites).

  **New rule — do not `import` a UI-root module from library code.**
  track-panels initially imported `eseq.mixer` for
  `muted?`/`track-color-*`/`track-collapsed-label`. `import` *evaluates* the
  target, and `mixer.lisp` registers `(effect-buffer "*mixer*")`,
  `"*patch-mixer*"` and `seq-mixer-mode` at top level — so 60 metal_seq
  tests that load only `ui/effects.lisp` suddenly grew a mixer buffer
  rendering against SEQ stubs they never provided (`substring` on a nil
  track name). Reverted to the pre-existing alias-mediated flat spellings
  (`mixer-v2-*`), with a comment at the use site. The boundary: a module
  whose top-level evaluation registers UI roots (`effect-buffer`,
  `define-mode`+`set-buffer-mode-for`, keymaps) is an *application* module —
  reachable during migration through its compat aliases only. The endgame
  fix is splitting such helpers into a library module (mixer's
  track-color/mute helpers are the first candidate); until then the four
  converted roots (mixer, browser, sequencer, arrangement) must not be
  import targets.

  **First converted-module import cycle, and manifest double-eval.**
  panel-widgets ↔ process-panel import each other; load-once
  (`declared_modules`) terminates the cycle correctly. But an import
  cascade now pre-loads later manifest entries (panel-widgets at
  `effects.lisp:11` pulls param-controls and process-panel), and the
  manifest's raw `load` lines then re-evaluate them — `load` deliberately
  has no dedupe (§4). Benign today (re-defs and alias registrations are
  idempotent; the affected files' defstates are module-internal), and the
  whole issue dissolves with the manifests at eseq-mods.9; worth knowing
  when reading startup traces, and a reason not to delay .9 long.

  Gate: 586 passed / 2 pre-existing. Running conversion count: 23 files.

  ### S3b wave 3 tally (2026-08-12) — effect-panels, custom-ui-runtime, instrument-modulation + 12 builtin panels

  Fifteen parallel conversions, gate green on the first run. The 12 builtin
  panels (`eseq.effects.builtin.{eq8, str8-delay, space-echo, multiverb,
  dimension, phaser-flanger, roar, filter-panel, dynamics, multiband,
  dj-mixer, filterbank}`) are near-uniform: almost everything `%`-private,
  one or two aliases for the flat `audio-fx.lisp` dispatch (plus the odd
  Rust test eval), imports of filter-core/param-controls/param-grid, and
  **every `:key` in all 12 is a subtree key** — zero Rust assertion
  rewrites across the whole set. `eseq.effects.instrument-modulation` (22
  defs; hazard (k) fired live: the stripped `source-type` collided with a
  local `let` head, renamed to `kind`). `eseq.effects.effect-panels` (18
  defs, 3 defwidgets flat, 10 aliases). `eseq.effects.custom-ui-runtime`
  (41 defs, 34 identity aliases — it IS the generated-custom-UI vocabulary;
  ~20 reads of the vanilla-pinned `synth-ui-current-*` family requalified
  to the escape-hatch spelling; second import cycle, sections↔runtime).

  Notable new findings:
  - **Rust tests that slice lisp source TEXT.** `state_values/tests.rs`
    `lisp_def_slice`s `effect-panels.lisp` between two literal
    `(def instrument-toggle-mods-view` / `(def instrument-mods-toggle-button`
    headers and evals the slice headerless. Those defs must keep flat names
    and alias-mediated bodies, and even an explanatory COMMENT containing
    the marker string hijacks the first-occurrence search. Grep a
    converting file's def headers against raw-string Rust before renaming.
  - Hazard (k) native/widget collisions are common on strips: `source` is
    a native (eq8, phaser-flanger renamed around it), `knob`/`toggle`/
    `gate-led` are builtin widget names (filterbank kept them `%`-private).
    The four-list sweep catches all of these — it is not optional.

  Gate: 586 passed / 2 pre-existing. Running conversion count: 38 files.

  ### S3b wave 4 tally (2026-08-12) — panel-frame, panel-bodies, custom-ui-controls, custom-effect-ui, compressor, tape, convolution-reverb

  Seven conversions, gate green first run. `eseq.effects.panel-frame` (19
  defs, identity-alias hub; its main work was rewriting SIX converted
  callers to imports — the cycle hub's bare back-edges from waves 1–3 all
  retired; two new mutual import cycles, panel-frame↔param-controls and
  panel-frame↔effect-panels, both load-once-terminated).
  `eseq.effects.panel-bodies` (45 defs, 8 identity aliases; **the
  codegen-re-def variant of hazard (m) named precisely**: the
  `custom-instrument-synth-ui`/`custom-midi-fx-ui`/`custom-audio-fx-ui`
  dispatchers are re-def'd headerless by `custom_ui.rs` on every custom-UI
  rebuild, so a module's bare call would strand on the first rebuild — all
  such calls use the `eseq.vanilla/` escape-hatch spelling; also caught a
  capture fixture that *redefines* `instrument-key-note-active?` headerless,
  correctly alias-covered since aliases apply to writes).
  `eseq.effects.custom-ui-controls` and `eseq.effects.custom-effect-ui`
  (generated-vocabulary hubs: identity aliases because ~20 generated
  `instruments/**/ui.lisp` and on-disk `effects/*/ui.lisp` files call them
  flat; runtime↔custom-effect-ui closes the third import cycle).
  `eseq.effects.builtin.{compressor, tape, convolution-reverb}` — tape and
  convolution-reverb are the first consumers of another builtin module's
  RENAMED spellings (`dyn/percent-knob` etc.), retiring dynamics' aliases
  down to `tape`-era debt.

  Gate: 586 passed / 2 pre-existing. Running conversion count: 45 files.
  Remaining in effects/: custom-ui-lego, audio-fx, instrument-panel,
  sampler-panel, modulator-panel (wave 5), buffers (wave 6).

  ### S3b wave 5 tally (2026-08-12) — custom-ui-lego, audio-fx, and the panel-trio cycle

  `eseq.effects.custom-ui-lego` (106 defs — the generated-custom-UI layout
  vocabulary: 83 identity aliases forced by ~40 generated
  `instruments/**/ui.lisp` files, `custom_ui.rs` codegen, and
  `ui_validate.rs`'s accept-list; 22 `%`-private; big `:refer`s keep ~200
  call sites byte-identical). `eseq.effects.builtin.audio-fx` (the
  dispatcher: imports all 15 panels at their post-rename spellings, and its
  conversion **retired ~19 dead aliases** across the panel files — the
  first alias-shrink payoff of the batch; kept only the three
  Rust-test-eval'd ones plus filter-core's). The
  instrument-panel/sampler-panel/modulator-panel **reference cycle was
  converted by a single agent** wiring the three-way imports directly —
  modulator-panel needed zero aliases (its one caller is converted), the
  other two kept identity aliases for Rust test evals, `buffers.lisp`, one
  capture fixture, and `sampler-reset-view`, which **production** Rust
  evals by name (`reactive_sync.rs:2369`). Second and third instances of
  the wave-3 text-slice hazard (`editor/tests.rs` greps for
  `(def sampler-param-knob`) — the def keeps its flat public spelling with
  a comment. The defhook corollary (hazard e) fired for real:
  `rack-macro-arm`'s bare hook calls became `(run-hook …)`.

  Gate: 586 passed / 2 pre-existing. Running conversion count: 50 files.

  ### S3b wave 6 tally (2026-08-12) — buffers.lisp, and effects/ is done

  `ui/effects/buffers.lisp` → `eseq.effects.buffers` (8 defs, 3 aliases —
  including the batch's first **mode-keyspace alias actually minted**,
  `seq-plock-panel-mode`, for headerless `step-buffer.lisp`'s
  `set-buffer-mode-for`; the stage-4 rung finally exercised in production).
  Its three `mode-bind-key` handler strings naming other modules' defs were
  rewritten **pre-qualified** (`eseq.effects.panel-widgets/delete-selected-effect`
  etc.) — `qualify_registration_name` passes qualified spellings verbatim
  and dispatch finds the qualified global, a cleaner shape than keeping an
  alias for a handler string. Being the last unconverted consumer, its
  conversion retired **9 more aliases** across instrument-panel,
  process-panel, panel-widgets and track-panels. The file registers
  `effect-buffer` roots at top level, so its header carries the wave-2
  warning: never import it.

  **effects/ is fully converted**: 39 of its 40 files carry module headers.
  Headerless by design:
  `effects.lisp` + `builtin-effects.lisp` (load manifests, dissolve at
  eseq-mods.9) and `effects/step-buffer.lisp` (pure side-effect root).
  Gate: 586 / 2 pre-existing; full `cargo test -p sequencer` 1720 / 2
  pre-existing on HEAD too (hardcoded-44100 lint, graph-variable-reset
  demo — verified in a clean HEAD worktree); eseqlisp 1581 green (patcher
  dylib tests flake under parallelism, pass isolated — pre-existing).

  Running conversion count: **51 files.**

  ### S3b wave 7 tally (2026-08-12) — the accessor OWNER and the mode rung

  The two conversions every earlier wave deferred, done together because
  `seq-grid-mode` consumes `seq-core-state`.

  **`ui/seq-core-state.lisp` → `eseq.seq-core-state` — the first accessor
  OWNER to convert, and the mirror of hazard (m) that no batch had
  validated.** 20 defs, 1 `%`-private, **0 renames**, 18 identity compat
  aliases, 1 vanilla pin. The result generalizes into a reusable pattern:

  - **An identity alias serves BOTH ladder rungs at once.** An unconverted
    vanilla caller matches the alias key flat; a *converted* module's bare
    reference qualifies against **itself**, misses, and falls to the same
    base-name rung. So a hub file — and this is the vanilla UI's shared-state
    hub, spelled flat by ~20 lisp files and several Rust call sites —
    converts with no renames and no caller edits at all. Recipe step 3's
    offer to strip prefixes should be **declined** for hub files: renaming
    buys nothing and churns every consumer.
  - **Which of an owner's names are alias-safe** (the useful restatement of
    hazard (m), now with a worked example of each): *functions* are safe
    (slots written once by their `def`, the heal never unlinks); *`defstate`*
    is safe (`state_bindings`, flat key, compile time — and the alias covers
    that keyspace); *write-once plain `def`* is safe (`page-size`); a
    **mutable plain `def` is not, and an alias does not rescue it.**
  - `cursor-step` is the file's only mutable plain `def` and is therefore
    **pinned to `eseq.vanilla`** rather than aliased. Two flat spellings
    force it, and neither is reachable by alias: production Rust reads it
    with `rt.global_value("cursor-step")`
    (`src/ui/state_values/param_fields_and_sync.rs:1299`), and a Rust test
    seeds it with a headerless `(def cursor-step N)`
    (`src/ui/host_commands/step_history.rs:1632`) — a re-def that strands any
    healed module slot on the previous cell.
  - **New pitfall, silent and easy to hit: pinning obliges you to requalify
    every *in-file* reference too.** Inside the owning module a bare
    `cursor-step` interns `eseq.seq-core-state/cursor-step`, a *different
    cell* from the pinned `eseq.vanilla/cursor-step`. All three in-file
    references were rewritten to the pinned spelling. Nothing errors if you
    forget; the owner simply reads and writes a private shadow.
  - `current-page` had no caller anywhere outside the file and is one of the
    three globals hazard (k) names as collision-famous, so it went
    `%`-private. `param-mode` and `current-step`, the other two, are widely
    consumed and stay public behind identity aliases.
  - **A benign four-list hit worth recording so later waves do not rename for
    it:** `current-step`, `cursor-num-steps` and `cool-off-follow` all appear
    in the `register_native` sweep list — but every registration is a
    `#[cfg(test)]` stub in `src/ui/state_values/tests.rs`, for VMs that never
    load the owning lisp file. A `list3-natives.txt` hit only matters when
    the registration is in **production** Rust; a test stub *of the very name
    you own* is the intended shape and must not trigger a rename.

  **`ui/seq-grid-mode.lisp` → `eseq.seq-grid-mode` — the mode keyspace's
  end-to-end acceptance test.** 26 defs, 18 identity aliases, imports
  `eseq.seq-core-state`. It is the only *defining* file for a mode whose
  keymap binds handlers it does not own, so all three rungs fire in one
  file — and **all three turned out to be pre-built infra that needed no
  changes**, which is the headline result:

  1. `define-mode` qualified the registry key to
     `eseq.seq-grid-mode/seq-grid-mode`; both flat
     `(set-buffer-mode-for … "seq-grid-mode")` callers reach it through the
     identity alias — `ui/step-grid.lisp` (headerless) on the flat rung, and
     `ui/sequencer.lisp` (converted) via qualify-against-self → miss → same
     base-name rung.
  2. `mode-bind-key` qualifies its *handler* string against the caller's
     module unconditionally, so the **seven handlers defined outside this
     file** (`cursor-left`, `cursor-right`, `select-all-steps`,
     `delete-selected-steps` ×2, `cursor-toggle`, `seqv-collapse-all-tracks`)
     became `eseq.seq-grid-mode/<name>` and landed on
     `resolve_handler_name`'s qualified→flat fallback — exactly what
     `module_mode_binding_dispatches_a_vanilla_handler` was written to pin.
  3. The eight `set-*-mode` handlers are defined here, so their bound strings
     qualify to slots this module owns: exact hits.

  `param-mode` is deliberately left **bare** despite its owner being
  imported: it is a `defstate`, whose flat-key `state_bindings` resolution is
  the documented and test-pinned path, whereas a *qualified write to another
  module's `defstate`* is a shape no test covers. Prefer the documented rung
  over the tidier-looking one.

  Gate: 586 / 2 pre-existing (metal_seq), 1720 / 2 (`sequencer --lib`),
  eseqlisp clean at `--test-threads=1` — the parallel
  `widget_render::patcher::tests::*writeback*` failures are user-confirmed
  flakes and cannot depend on `ui/*.lisp`.

  Running conversion count: **53 files.**

  ### S3b waves 8-10 tally (2026-08-13) — slice 3 is COMPLETE

  Thirteen files in three parallel waves, each gated at baseline. **Every
  non-fixture file under `crates/sequencer/ui` now carries a module header**
  except the deliberate exclusions listed below.

  - **wave 8** — `eseq.step-grid-interactions` (71 defs, 42 identity aliases,
    12 pins), `eseq.seq-script-picker` (39 defs, 9 pins), `eseq.step-grid`.
  - **wave 9** — `eseq.transport` (the largest file, 863 lines),
    `eseq.piano-roll`, `eseq.bus-grid`, `eseq.seq-panels`,
    `eseq.seqv-track-params`.
  - **wave 10** — `eseq.agent`, `eseq.macros`, `eseq.patch-macros`,
    `eseq.seq-step-tabs`, `eseq.legacy.mixer`.

  **The import-retires-aliases cycle, demonstrated.** `eseq.step-grid`
  imported `eseq.seq-grid-mode` and retired **15 of the 18** aliases wave 7
  had minted — the whole point of S3b. The 3 survivors are load-bearing: the
  mode identity alias, and `double-`/`halve-track-pattern`, which production
  `input.rs` evals by flat name. One of the retired names (`param-color`) had
  no caller at all: wave 7 over-minted it, which is the expected failure mode
  of sweeping *before* the consumer converts.

  **Pins are a two-file property, not a Rust property.** The eleven mutable
  drag-state globals in `eseq.step-grid-interactions` are pinned not because
  Rust spells them but because `ui/bus-grid.lisp` reads *and* `set!`s all
  eleven for the bus-lane gestures — genuinely one shared gesture state
  across two files. When bus-grid converted in wave 9 the pins did not
  retire; both modules now spell the same `eseq.vanilla/` slot, and they can
  only retire if both files are requalified together. Restatement of the
  rule: **an alias can never rescue a mutable plain def, no matter who owns
  it.**

  **Three corrections to earlier belief, all worth carrying forward:**

  1. **`bind-key` is not a reason to alias.** `Runtime::bind_key` calls
     `qualify_registration_name` exactly like `mode_bind_key`
     (runtime.rs:864-867), so a handler defined in the binding module is an
     exact hit. A wave-9 conversion asserted the opposite in its header and
     minted an alias for it; both were removed after the two global bindings
     in `eseq.step-grid-interactions` were verified working with none. §2's
     "late-bound string handlers capture their module" was right all along.
  2. **`(def x (state …))` is a `defstate` in disguise** — see hazard (m).
     `patch-macros-filter` was pinned, then correctly reverted to an alias.
  3. **The `metal_seq_core_lisp_files_parse` gate list had gone stale.**
     Eighteen files carrying module headers were absent from it, so the cheap
     gate silently proved nothing about them — several conversions in these
     waves ran it, passed, and had to be told it was meaningless for their
     file. All converted modules are listed now, with a comment saying to keep
     it that way.

  **Headerless BY DESIGN, and why** (this is the final answer for slice 3):
  - `ui/main.lisp`, `ui/effects.lisp`, `ui/builtin-effects.lisp` — load
    manifests; they dissolve at `eseq-mods.9`.
  - `ui/effects/step-buffer.lisp` — pure side-effect root.
  - `ui/themes.lisp` + the ten `ui/themes/*.lisp` — each is a flat bag of
    theme-slot assignments with no callable surface to namespace.
  - the 58 `ui/capture-fixtures/*.lisp` — test fixtures that deliberately
    exercise the *flat* caller path, which is precisely what the compat-alias
    rungs exist to serve; converting them would delete that coverage.
  - **the ~305 lisp files outside `ui/`** (`instruments/**`, `effects/**`,
    `scripts/**`, `midi-fx/**`, `defmacros/**`) are user and generated
    CONTENT and stay headerless permanently. They are the reason a large
    block of identity aliases can never retire — `eseq.macros`' 31, the
    `eseq.seq-script-picker` host→script contract pins, and
    `eseq.effects.custom-ui-lego`'s 83. Alias-table shrinkage has a floor,
    and this is it.

  Gate: metal_seq 586 / 2 pre-existing, `sequencer --lib` 1720 / 2, eseqlisp
  clean at `--test-threads=1`.

  Running conversion count: **66 files — slice 3 done.**

  ### Content call-site sweep preflight (eseq-mods.11, 2026-08-13)

  The ratified follow-up reverses the earlier "hard floor" decision: old
  spellings in repository content are migration debt, not API. The 720
  `module-compat-alias` forms are now extracted by
  `tools/migrate_module_aliases.py` into one durable old→fully-qualified table
  (`tools/module-compat-aliases.tsv`). All 720 old names have exactly one target;
  there are no conflicting mappings.

  The token-aware preflight scanned the real tracked content set rather than the
  old ~305-file estimate: 210 eligible files after protecting the author's dirty
  directories (73 instruments, 30 effects, 10 defmacros, 6 MIDI FX, 27 scripts,
  and 64 capture/theme files). **135 files contain 6,177 old-name occurrences**:
  5,559 in 66 instrument files, 539 in all 30 effect UIs, 28 in 21 scripts, 11
  in one MIDI FX UI, and 40 in 17 capture fixtures. Defmacros and themes have no
  hits. The protected `instruments/monomachine/vox/ui.lisp` is a separate
  101-occurrence follow-up.

  **Sharp-edge triage for this corpus:** all 6,175 executable occurrences are
  whole Lisp symbols, never substrings; the other two are backtick-delimited
  documentation references in instrument comments. There are **zero old names
  in strings and zero in quoted data**, so mode-handler strings and symbolic
  widget-key strings need no special rewrite in this repository sweep. Symbols
  used as prop values (notably the `ui-accent-*` and `ui-lego-*` vocabulary) are
  evaluated globals and therefore qualify like calls. Matching and rewriting is
  lexer-based, so `seq-x` cannot touch `my-seq-x`; strings and quoted forms are
  rejected for manual triage rather than guessed.

  **`defstate` exposure is concentrated and testable:** 25 occurrences spanning
  17 renamed states in 10 capture fixtures (effect-modulation, rack panel,
  macro-mapping, scene-push, arrangement, and instrument-panel state). These
  must become direct qualified reads/writes so the fixture and rendered module
  share the same reactive node after aliases disappear. Capture those fixtures,
  rather than treating a successful parse as evidence that the value flowed.

  ### Content call-site sweep outcome (eseq-mods.11, 2026-08-13)

  Rewritten and reader-parsed: 66 instrument files (5,559 symbols), all 30
  effect UIs (539), one MIDI FX UI (11), 21 scripts (28), and 17 capture
  fixtures (40). Defmacros and themes had no old spellings. The acceptance
  checker reports **zero** over all 210 eligible tracked files. The protected
  `instruments/monomachine/vox/ui.lisp` remains a separately tracked 101-hit
  follow-up (`eseq-mods.14`); no other protected dirty root has a tracked
  in-scope hit.

  Capture verified the risky qualified-state paths: macro mapping highlights,
  rack macros, scene push, arrangement trim ghost, lit instrument keys, and the
  Multiverb modulation pane all rendered from their direct qualified writes.
  The Multiverb fixture also exposed and fixed a pre-existing incomplete
  identity setup: it now sets the effect track and rack-slot fields as well as
  chain/slot/bus, so the intended mods pane actually opens. Three representative
  rewritten instrument families passed `instrument_probe` with finite nonzero
  output. `tools/audition/audition.py` is currently unusable independently of
  this sweep: it still reads the removed `src/lisp_host.rs`, then expects the
  retired `dgen-c-v2-host-sample-rate` ABI instead of `dgen-host-abi-v1`.

  The compatibility-table deletion is intentionally still blocked. Exact
  whole-file Rust-string accounting finds 425/720 aliases still named by Rust:
  383 in `state_values/tests.rs`, 12 in the `.10`-listed codegen files, and 165
  in other Rust (with overlap); 295 have no Rust string reference. Across all
  repository Lisp/Rust consumers outside each alias's own declaration, 613
  aliases remain referenced and 107 are now repo-unreferenced. That is `.10`'s
  remaining work, not this sweep. External user content is protected by the
  dedicated migration-tool blocker `eseq-mods.13` (pre-load warning plus
  explicit dry-run/atomic rewrite); silently rewriting user files was rejected.

  ### What remains for slice 3

  **Nothing — slice 3 is complete.** Every non-fixture file under
  `crates/sequencer/ui` carries a `(module …)` header except the
  headerless-by-design set enumerated in the waves 8-10 tally above. The three
  items this section used to track are all closed:

  - the **~70 unconverted files** figure was a stale estimate carried forward
    from before the effects batch; the real remaining set was 15 files.
  - the **`seq-grid-mode.lisp` infra gap** is closed, and the answer was that
    there was no gap: all three mode rungs were pre-built, and the seven
    foreign `mode-bind-key` handlers needed no work at all.
  - the **accessor-owner question** is answered. Converting an owner is safe,
    and cheaper than feared: identity aliases serve both ladder rungs at once,
    so a hub converts with zero renames and zero caller edits. Only *mutable
    plain defs* need pinning.

  - **The compat-alias table is at 720 entries** across the 65 converted files
    (it was 186 when the "big four" landed; effects/ and slice 3's hub
    conversions account for the rest, most of them *identity* aliases minted
    so that hub files could convert without touching a single caller).
    Deletion criteria, concretely: an alias may be dropped when (1) no
    unconverted lisp file spells the old name, (2) no Rust source spells it —
    including `#[cfg(test)]` harnesses and multi-line raw-string lisp, which is
    what the whole-file sweep is for, and (3) for the identity aliases
    (`sample-browser-here`, `seq-ctrl-g`, `choose-model`), the by-name Rust
    caller has been taught the qualified spelling. Criterion (2) is the binding
    one: most surviving aliases exist for `state_values/tests.rs`, so the table
    shrinks fastest by requalifying test entry points, not by converting more
    files. The six vanilla-pinned names in `browser.lisp` are **not** alias-table
    entries and do not shrink with it — they fold in only when the Rust codegen
    in `host_commands/{scripts,tracks,instrument_authoring}.rs`,
    `ui/event_loop.rs` and `ui/edit_sessions.rs` emits qualified names, which is
    a slice-4/5 item.

    **External content no longer imposes a hard floor (eseq-mods.13).** The
    durable 720-entry TSV is embedded in `eseqlisp`; runtime protection does
    not read `tools/`, derive mappings from Lisp forms, or depend on
    `module-compat-alias` continuing to exist. A single token-aware lexer is
    shared by detection and migration. It matches complete Lisp symbols, so a
    longer hyphen-prefixed symbol cannot collide; identity aliases still
    qualify; and executable `defstate` reads and writes receive the same direct
    qualified replacement. Strings and quoted data are diagnostics for manual
    review, never guessed rewrites.

    Path-associated ESeqLisp converges on `VM::eval_module_source`: this covers
    `load`/`import` through `SourceManager`, editor evaluation and hot reload,
    explicit out-of-checkout paths, scratch and project scripts, and capture
    scripts. A process-wide canonical-path set emits at most one warning per
    file per app session even when a hot-reload pass evaluates it repeatedly.
    The warning includes the total occurrence count, the first five
    path:line:column old→qualified hits, and exact dry-run and write commands.
    Clean scans are not memoized, so introducing an old name in a later edit is
    still detected. The scan is one lexer pass and happens before normal parse
    and evaluation; detection never blocks loading while aliases remain. Metal
    Seq explicitly excludes only its checked-in `crates/sequencer/ui` factory
    root, whose module-local bare names are valid and whose repeated startup
    pass is unnecessary. Authored instruments, effects, MIDI FX, and scripts
    remain outside that exclusion. Exclusions are registered roots rather than
    checkout heuristics, so an arbitrary out-of-repo path cannot be mistaken
    for factory content.

    Four authored-source paths transform or concatenate files before the VM
    sees their real provenance, so they call the same scanner before doing so:
    custom instrument `ui.lisp`, custom audio-effect `ui.lisp`, custom MIDI-FX
    `ui.lisp`, and the MIDI-FX control-Lisp library. Script discovery needs no
    special hook because execution reaches the VM chokepoint. Instrument and
    audio-effect `dsp.lisp`, and `defmacro_library.rs`'s `macro.lisp` packages,
    are DGenLisp rather than ESeqLisp and are deliberately outside this alias
    vocabulary. There are not yet `~/.eseq.d` or content-tier roots; future
    roots that use normal path-associated evaluation inherit the VM preflight
    without another integration.

    The user-facing vehicle is the small Rust binary
    `eseqlisp_migrate_module_aliases`, rather than the developer-only Python
    sweep. This keeps the embedded dictionary and lexer identical to load-time
    detection and requires no Python runtime in an app bundle. Invocation must
    choose exactly one of `--dry-run` or `--write`; omitting the mode is an
    error, so migration is never silent. Dry-run prints a unified diff per
    changed file and performs no writes. Write mode stages and syncs every
    changed file beside its destination before mutation, then uses adjacent
    rename/backup pairs; any replacement failure rolls the already-committed
    files back, preventing a half-migrated tree. A second run reports zero
    replacements. String and quoted-data diagnostics retain path, line,
    column, old spelling, and target, and remain untouched in both modes.

  ### Compat-alias retirement (eseq-mods.10 end state)

  The migration table and `(module-compat-alias …)` form no longer exist.
  Pins made with explicit `eseq.vanilla/…` definitions remain a separate §3
  escape hatch, and headerless-content fallbacks remain supported. The final
  resolution ladders are:

  - Compiler globals: lexical scope → declared current-module entry →
    `:refer` target → current/implicit-module entry → flat entry → intern the
    current-module spelling. Qualified core and `eseq.vanilla` reads may fall
    back to their flat native/pinned entry.
  - Runtime by-name globals: qualified exact → qualified core/vanilla flat
    fallback; bare reactive namespace flat entry → `eseq.vanilla/<name>` →
    flat entry. Once an index is selected, the effective-read layer checks a
    qualified override before returning the factory cell; an empty override
    registry is a single branch with no hash lookup.
  - Cached `LoadGlobal`: cached index → effective override dispatcher (when
    present and healthy) → factory cell → late-binding heal. The dispatcher's
    `original` handle bypasses the effective layer and resolves the current
    factory cell by qualified name at each call.
  - Late binding: an empty qualified slot may heal through its
    `eseq.vanilla/<base>` spelling (for non-vanilla callers) and then the flat
    base. An empty flat slot has no cross-module fallback.
  - Macros: current module → `:refer` → flat. Qualified names are exact (with
    the existing flat fallback for explicitly qualified legacy/core keys).
  - `defstate`: exact/explicit-vanilla key → current module → `:refer` → miss;
    runtime tracked-state lookup is exact/explicit-vanilla → executing module
    → miss.
  - Modes: exact → qualified-to-flat vanilla fallback → miss.
  - Completion is derived from real global keys only. Qualified module names
    remain visible; only `eseq.vanilla/` is displayed as bare.

  The durable TSV, load-time detector, and explicit atomic migration command
  from eseq-mods.13 remain independent safety infrastructure for external
  content. They deliberately outlive the runtime compatibility mechanism.

- **Slice 4 — `defhook` + init inversion + `override` (BUILT 2026-08-13).**
  The four `macro-mapping-*-hook` stubs are `defhook` declarations, dead
  ordering comments are gone from `main.lisp`, and `~/.eseq.d/init.lisp`
  loads transactionally after every factory/content root and hot-reloads from
  outside the repo. `override` (§6.1) is a name-keyed snapshot-aware registry
  like `extension_hooks`; cached global reads add its effective-value rung.
  Acceptance test:
  `user_init_boot_proves_hook_mx_theme_and_visible_around_override`.
- **Slice 5 — packages.** Manifest format, load path, author scoping,
  `defcustom`; generalize `defmacro_library.rs`.

Warnings-not-errors where hackability matters: redefining a symbol owned by
another module warns (this is the tooling that would have caught both
duplicate-definition bugs in §1); qualified references to non-exported symbols
warn; `:refer` of a non-exported symbol is an error.

## 11. Open questions

1. **dgenlisp scope.** This spec covers the UI lisp. Does `use-defmacro`
   (patcher/dgen) migrate to `import`, or stay as-is with the package layer
   shared? The materialize-by-inlining mechanism
   (`defmacro_library.rs:525-538`) works and may not be worth disturbing
   until packages exist.
2. **Reactive namespace collision rule.** `SEQ.playing` vs a hypothetical
   module named `SEQ` — reserve all-caps single-segment names for reactive
   namespaces, or check `vm.reactive_namespaces` at resolution?
3. **`effect-buffer` names** (`"*sequencer*"`, `"*fx*"` — Rust asserts on
   the exact set, `editor_setup.rs:123-135`): qualify them, or treat buffer
   names as deliberately global UI surface? Leaning global (they are
   user-visible buffer identities, like Emacs buffer names).
4. **Hot-reload interaction.** When a module file changes, `owner_root_for`
   (`hot_reload.rs:315-345`) re-evaluates from the owning root. With
   `import`-declared edges the graph is precise; confirm re-evaluation
   re-registers hooks without duplicating `add-hook` entries (likely:
   `add-hook` is idempotent per (hook, module, key)).
5. ~~"Hook" vocabulary collision~~ — resolved, see §6: the clock-callback
   `register-hook` system is deprecated for removal; `defhook`/`add-hook`
   own the word.

Related: `docs/big-file-split-plan.md` (the Rust-side analogue of slice 2),
`crates/eseqlisp/src/defmacro_library.rs` (package format precedent),
`crates/eseqlisp/src/hot_reload.rs` (ModuleGraph this builds on),
`crates/sequencer/ui/README.md` (current `@/` load convention).
