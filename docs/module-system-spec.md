# Module System — Namespaces, Imports, and Packages for eseqlisp

Status: rev 3, 2026-08-11 — surface syntax locked (`module` / `import` /
`/` qualifier / `%` private); slice 1 scoped with the sdf stdlib conversion
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

(defstate panel-visible false)

(def track-strip (i)
  (if (tc/track-collapsed? i) (%collapsed-strip i) (full-strip i)))
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
4. **`%name` marks private.** A leading `%` on a definition means internal:
   visible and callable from outside (hackability — a warning, not a wall),
   but qualified references to another module's `%`-symbols emit a compiler
   warning. Self-documenting at both definition and call site, survives
   copy-paste, needs no metadata syntax. Rejected: `^:private` (metadata
   system we don't have), a `(private …)` block (information invisible at
   call sites).
5. **Function syntax is `(def name (args) body)`** — this spec's examples
   use the real eseqlisp form (e.g. `(def track-peak (i) …)`,
   `ui/mixer.lisp:6`), not Scheme-style `(def (name args) …)`.

## 3. Resolution semantics

Bare reference inside a module resolves in order: lexical scope (locals,
upvalues — unchanged) → current module → `:refer`red symbols → core prelude.
Qualified reference `X/name`: `X` resolves as an import alias first, then as
a full module name. Unknown alias/namespace is a **compile error** at load
time, not a runtime surprise.

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
file (§7), evaluates it if and only if it has not been evaluated, and records
the dependency edge in the existing `ModuleGraph` (edges currently inferred
from observed `load`s at `hot_reload.rs:282-290` become declared). This
dissolves the hand-maintained ordering in `ui/main.lisp` (the "define before
loading render roots" comments, and `track-collapse.lisp` being raw-loaded
three times from `browser.lisp:5`, `mixer.lisp:4`, `sequencer.lisp:5`).

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
  replaces rather than stacks.
- Global **resolution checks the override registry before the owning def**
  — one extra lookup at the same `resolve_symbol`/`ensure_global` choke
  points. Because the override is a separate layer, the owner module
  re-evaluating refreshes the factory definition *underneath* it without
  disturbing it. Load order stops mattering.
- `:around` receives the *current* underlying def as `original` at call
  time (not captured at override time), so the wrapper composes with
  factory updates.
- `(remove-override eseq.mixer/track-strip)` is "revert to factory." The
  inspector can show provenance: *track-strip — overridden by
  ~/.eseq.d/init.lisp*.
- **Graceful failure:** an override whose body errors at call time logs
  (gated like hook-listener errors) and falls through to the factory def.
  A broken user override degrades one component; it never bricks the app.
- Overriding a `%`-private symbol warns, exactly like a qualified `%`
  reference (§2, decision 4): privates are the unstable rung, and the warning
  enumerates which overrides will break on update. Overriding *public*
  defs is the semi-stable API surface — `%` is the lever that keeps that
  surface deliberately small.

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

## 9. The sdf pilot (slice 1 acceptance test)

`sdf-stdlib.lisp` already hand-rolled this exact convention as flat strings:
~17 defmacros literally named `sdf/circle`, `sdf/rounded-rect`, `sdf/rotate`
etc. (`crates/eseqlisp/sdf-stdlib.lisp`, loaded at `Runtime::new` via
`include_str!`, `runtime.rs:1245`), plus `sdf/layer`/`sdf/fill`/`sdf/paint`
as Rust-registered builtins referenced from lisp-in-Rust template strings
(`runtime.rs:572-600`, `lib.rs:2190`). ~34 lisp files consume `sdf/*`.

Conversion: the file gains `(module sdf)` and the defmacros drop their
prefixes; internal fill-shape macros (`__hslider-fill`,
`__vslider-fill-with-material`) become `%hslider-fill` etc.; the Rust
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
  per-file with its module header.
- **Slice 4 — `defhook` + init inversion + `override`.** Convert the four
  `macro-mapping-*-hook` stubs, delete ordering comments from `main.lisp`,
  add `~/.eseq.d/init.lisp` loaded last. `override` (§6.1) lands here — it
  is a name-keyed snapshot-aware registry like `extension_hooks`, and the
  init inversion is what makes user overrides load in the right place.
- **Slice 5 — packages.** Manifest format, load path, author scoping,
  `defcustom`; generalize `defmacro_library.rs`.

Warnings-not-errors throughout the migration: redefining a symbol owned by
another module warns (this is the tooling that would have caught both
duplicate-definition bugs in §1); referencing another module's `%`-symbol
warns; `:refer :all`, if ever added, is loud and greppable.

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
