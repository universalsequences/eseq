# Module Exports — private-by-default visibility for eseqlisp modules

Status: **BUILT**, rev 1. Shipped 2026-08-20. Supersedes `%`-prefix privacy
(module-system-spec.md §2 decision 4).

Parent spec: `docs/module-system-spec.md` (rev 3). This document only
changes how a symbol's visibility is *declared*; resolution (§3), import
semantics (§4), and `override` (§6.1) are unchanged except where noted.

## 1. Motivation

The `%`-prefix convention did its job during migration — it forced a
visibility decision onto every one of the ~1,176 internal symbols
(≈3,848 occurrences in `content/ui`) — but it puts the ugliness
at every definition *and* call site, permanently. In Common Lisp, where
the convention originates, `%` marks "low-level, unsafe, holding the
knife by the blade," and its visual hostility is the point; using it for
ordinary private helpers (most of a module) inverts that ergonomics.
Languages that enforce visibility mostly do it without a name sigil:
export lists (CL `defpackage :export`, Racket `provide`, Erlang
`-export`, R7RS libraries) or definer keywords (Rust `pub`, Clojure
`defn-`). We adopt the export-list model: **private by default, public
symbols declared in `export` forms**, the mirror image of `import`.

What `%` bought us is exactly what makes leaving it cheap: every symbol
is already labeled, so the export lists can be generated mechanically
(§6).

## 2. Surface syntax

```lisp
;; ui/mixer.lisp
(module eseq.mixer)

(import eseq.track-collapse :as tc)
(import eseq.seqv :refer (cursor-step))

(export track-strip
        track-color
        set-track-mute)

(defstate track-menu-open false)          ; private: not exported

(def track-strip (i) ...)                 ; public: exported above
```

Decisions:

1. **`(export <sym> …)` is a top-level form**, sibling of `import` —
   same grammar family, same "one concept per top-level line" feel.
   Bare unqualified symbol names only; no strings, no qualified names
   (but see §5 re-export reservation). Applies uniformly to every
   definer: `def`, `defn`, `defstate`, `defmacro`, `defhook` — anything
   that creates a module-level global.
2. **Multiple `export` forms union.** A small module writes one block
   under the imports; a large module may export per-section next to the
   relevant definitions. The linter nudges toward one block at the top,
   same posture as imports (parent spec §2 decision 2).
3. **Position-independent.** An `export` form may appear anywhere at
   top level, before or after the definitions it names. Validation runs
   at end-of-module evaluation (§3), so forward references are fine and
   the loader does not care about ordering. `export` inside a function
   body is an error, mirroring `import`.
4. **Private is the default.** A definition not named by any `export`
   form is internal to its module. No sigil, no metadata; the name is
   just a name.
5. **No renamed exports.** Racket's `rename-out` makes grep lie; a
   symbol is exported under its defined name or not at all.
6. **Headerless files (`eseq.vanilla`) export everything.** The
   implicit module (`modules.rs:14`, parent spec §10 slice 0) predates
   visibility and stays fully public — `export` in a headerless file is
   a compile error ("declare a module first"). Core namespaces
   (`modules.rs:18`) are likewise fully public; namespaced Rust natives
   are implicitly exported by registration.

## 3. Semantics

**The export set.** Evaluating a module accumulates its export set from
all `export` forms in the unit. When the module's evaluation completes,
every exported name must have been defined in that module — a missing
one is a **load-time error** naming the symbol and the `export` form's
location. This kills the classic drift failure of centralized export
lists (add a definition, forget the export → confusing unbound error at
some distant call site; here the inverse — export a deleted symbol —
fails loudly at the source).

**Consumer-side enforcement** happens at the existing compile-time choke
points (`Compiler::use_global` / `resolve_symbol`, compiler.rs:1095) and
splits by reference shape, exactly parallel to today's `%` rules:

1. `(import M :refer (name))` where `M` does not export `name` — **hard
   compile error** at the import site. `:refer` is the "I want this
   bare in my namespace" contract; an unexported symbol is not offered.
2. Qualified reference `M/name` where `name` is not exported — **compiler
   warning**, same severity and phrasing slot as today's cross-module
   `%` warning (compiler.rs:1275). Hackability is preserved by design
   (parent spec §2 decision 4's rationale survives even though its
   mechanism doesn't): another module's internals stay visible and
   callable, with a warning, not a wall. The qualified spelling already
   self-documents the boundary crossing at the call site — which is also
   why `:refer` (which hides the crossing) errors instead. Ratified
   2026-08-19.
3. `override` (§6.1) targeting a non-exported symbol — **warning**, replacing
   the `%`-private override warning (compiler.rs:1979).
4. Same-module references — unaffected. Privacy is a module boundary
   concept only.

**Load-order interaction.** Enforcement requires the exporter's export
set, which is available whenever the consumer got the name via `import`
(import evaluates the module first, parent spec §4). The dotted-namespace
escape hatch (referencing a module not yet evaluated, parent spec §3)
already only warns "unknown module"; it additionally skips export
checking — you cannot check a set that does not exist yet. Alias-shaped
references always have the set.

**Runtime by-name lookups are exempt.** Rust-host resolution
(`VM::resolve_global_read_index`, `input.rs`-style eval-by-name, the
`state_values` test drivers) bypasses visibility entirely — enforcement
is compile-time only, the VM and global table do not change. This is
load-bearing: several `mixer-v2-*` entry points and `seq-ctrl-g` are
driven by name from Rust (`ui/mixer.lisp` header comment) and must keep
working regardless of export status, though such host-driven names
should normally be exported as documentation of the seam.

**Hot reload.** Re-evaluating a module rebuilds its export set from
scratch (a deleted `export` line takes effect on reload). Consumers are
not re-checked until they themselves recompile — acceptable, since
enforcement is warnings-and-import-errors, not runtime behavior. No
`ModuleRecord`/`ModuleGraph` changes beyond storing the export set per
module.

**REPL.** The identical `export` form works at the REPL against the
current module, appending to its export set (consistent with `import`'s
REPL story, parent spec §2 decision 2).

## 4. What happens to `%`

`%` has no semantic meaning: `is_private_name` and its two warning sites
were deleted at migration end (§6 step 4). The character remains legal in
names (`is_valid_module_name` allows
it) but the vanilla distro won't use it. It returns to being available
for a future CL-style "genuinely dangerous internals" *convention* if we
ever want one — convention only, enforced by nothing.

## 5. Reserved, not built

- **Re-export.** Facade modules (curating a public API over several
  implementation modules) will eventually want `(export (from
  eseq.mixer.strips track-strip))` or similar. Not designed now; the
  bare-symbols-only rule in §2.1 deliberately leaves the grammar room
  (any list-shaped element of an `export` form is an error today,
  reserved for this).
- **Export-all during authoring.** `(export :all)` for scratch modules
  was considered and rejected: headerless files already provide the
  no-ceremony tier, and `:all` in a named module is how APIs rot.

## 6. Migration

**Completed 2026-08-20.** Staged so the semantics flip and the mass rename never land in the same
change, following the alias-era playbook (parent spec §10).

1. **Land `export` + validation, `%` keeps meaning.** Both mechanisms
   coexist: a module with no `export` forms keeps today's behavior
   (everything public except `%`-names). A module with at least one
   `export` form opts into export semantics for public/private
   determination; its `%`-names are just names with an ugly prefix.
   Per-module opt-in = per-module conversion, no flag day.
2. **Mechanical conversion, one module (or family) per commit.** A
   script (`tools/` bin, warn+migrate posture like the alias-era `.13`
   bin) that for each module: (a) collects unprefixed module-level
   definitions → emits the `(export …)` block under the imports; (b)
   strips `%` from every occurrence of the module's own private names,
   module-scoped; (c) refuses to touch a module where stripping would
   collide with an existing bare definition and prints the pair.
   Global upper bound on collisions measured 2026-08-19: **25 candidate
   names** (e.g. `%selected-track`/`selected-track`, `%close`/`close`)
   across the whole tree; per-module true collisions will be fewer and
   are resolved by hand-renaming the private one first.
3. **Convert cross-module `%` references.** Any surviving qualified
   reference to another module's former `%`-name (the warned-but-legal
   hackability cases) gets the stripped spelling in the same commit as
   its owner's conversion — the script rewrites qualified occurrences
   tree-wide for the module being converted.
4. **Delete `%` semantics.** Remove `is_private_name` and both warning
   sites; every named module now requires export semantics (a named
   module with zero `export` forms simply exports nothing, which
   becomes meaningful rather than "legacy mode"). Update
   module-system-spec.md §2 decision 4 to point here.

Rust-side touch points, expected exhaustive: `modules.rs` (export-set
storage + validation helpers), `compiler.rs` (the two warning sites,
`:refer` error, `export` form compilation), `vm.rs`
(`eval_module_source` end-of-unit validation hook), plus the linter
nudge. The VM's flat global table, name mangling, and resolution ladder
are untouched.

## 7. Open questions

1. ~~Severity for qualified access to non-exported names~~ — resolved
   2026-08-19: warning, for hackability (§3.2). A stricter mode can be
   revisited if a real bug motivates it.
2. **`defstate` visibility vs reactive namespaces.** A non-exported
   `defstate` is invisible to other modules' *code*, but its reactive
   identity (binding tables, `bind-graph`) is unchanged. Confirm no
   Rust-side reactive machinery resolves defstate cells by qualified
   name in a way that should respect exports (believed none — runtime
   lookups are exempt by §3).
3. **Completion/tooling.** Should editor completions for `M/…` rank or
   filter by export status? Nice-to-have, not part of the slices above.
