# Procedural Macros Spec

Upgrade eseqlisp `defmacro` from template substitution to procedural expansion:
a macro body is code that *runs at compile time*, receives the call's forms as
data, and returns the expansion. Combined with three discipline rules
(expand-before-ship, pure expanders, deterministic gensym), this makes
authoring-side rewriter packages (`alez.jaki`, `alez.sig`, and everything that
wants to follow them) expressible as plain macros — no `eval`, no source-string
building, no dual-VM loading of macro layers.

## Motivation

Two shipped packages already need compile-time computation and work around its
absence the same way:

- `alez.jaki.surface/jak` expands to a runtime function (`channel-register`)
  that walks the quoted body, then builds scheduler source **as a string** and
  hands it to `def-sequencer :tick-source`.
- `alez.sig.surface/sig` expands to a runtime function (`sig-register`) that
  parses options, rewrites its pipeline vocabulary into scheduler-safe
  arithmetic, then builds a `def-process` **as a string** and `eval`s it.

Both exist because `defmacro` today is pure quasiquote substitution
(`compiler.rs expand_quasiquote`): a macro can splice argument forms into a
skeleton but cannot compute — no option parsing, no tree rewriting, no name
construction at expansion time. The computation is forced into runtime
functions, and a runtime function cannot reach `def-process`'s syntactic
auto-quote capture, so it must round-trip through source text.

With procedural macros, `sig` collapses to:

```lisp
(defmacro sig (name &rest spec)
  (let ((opts (parse-opts spec)))
    `(def-process ,(str "__sig-" name)
       :every (beats ,(get opts :rate))
       :run (let ((phase (mod (+ (/ (now-beats) ,(get opts :over))
                                 ,(get opts :from)) 1)))
              (send ,name ,(rewrite (get opts :pipe)))))))
```

The expansion is a literal `def-process` form, so the existing auto-quote
capture fires naturally and the string/`eval` layer disappears.

## Design principle

Expansion is a **pure, deterministic, authoring-side** function from syntax to
syntax. Every rule below serves one of those three words. If a proposed
extension needs expansion to observe runtime state, run on the scheduler VM, or
produce different output on re-eval, it is out of scope by construction.

## Kernel change: evaluated macro bodies

`defmacro` keeps its surface syntax: `(defmacro name (params… [&rest r]) body)`.
The change is what happens at expansion:

- **Today**: `expand_quasiquote(body, bindings)` — substitute bound params into
  the quasiquote skeleton; unquoted non-params pass through as-is; no code runs.
- **After**: the body is compiled once (at `defmacro` definition) into a VM
  function. Expansion **calls** it with each parameter bound to the
  corresponding argument *form as data* (`&rest` gets the remaining forms as a
  list). The returned value is the expansion, itself re-expanded until fixpoint
  (the existing loop in `expand_macros` already does this, with its existing
  depth limit as the recursion backstop).

Forms-as-data uses the existing expression⇄value round-trip (`source`, quoting):
symbols arrive as symbols, lists as lists, so `first`/`rest`/`nth`/`cons` and
quasiquote all work on them unchanged.

### Compiler ⇄ VM seam

The compiler today owns no evaluator — that is precisely what substitution-only
expansion bought. Procedural expansion adds one callback: the compile path gets
a handle through which it can invoke a compiled macro function in the owning
runtime's VM. Definition order within a compile unit is preserved: a `defmacro`
is compiled and registered the moment it is encountered, so later forms in the
same unit can use it (matching today's behavior).

An expander that throws aborts the compile of that form with a diagnostic
attributed to the macro *call site* (see Source mapping).

### Backwards compatibility

Existing macros are quasiquote templates. Under evaluation, a body that is one
quasiquote with only bound-parameter unquotes evaluates to exactly what
substitution produced — the common case migrates for free. The behavioral
difference: an unquote of a **non-parameter** expression is today passed
through unevaluated (`expand_quasiquote_inner`'s fallback) and will now be
**evaluated at expansion time**. A lint pass over checked-in `defmacro`s
(content, packages, defmacro library) must find any macro relying on the old
fallback before the switch; the patcher's textual defmacro machinery
(`defmacro_library.rs`) is in scope for the audit but its macros are simple
templates and expected to pass unchanged.

## Rule 1: Expand-before-ship

All bodies that cross the VM boundary ship **post-expansion**. At every
capture site that auto-quotes a body into shipped source — `def-process`
clause values, `every`/`after`/`on`/`tap` bodies, `def-sequencer`
`:tick`/`:init` and graph mode — the compiler macro-expands the captured forms
*first*, then quotes and serializes the residue.

Consequences:

- The scheduler VM never expands macros; shipped source is macro-free kernel
  forms. The "which VM has the macro loaded?" question is deleted, not solved.
- Expansion-time code runs only on the authoring VM, which already owns every
  package, module, and buffer definition.
- `alez.jaki.core/pat` no longer expands scheduler-side, shrinking the
  scheduler's package dependency to runtime functions only (`run`, `eval-at`).

This tightens, and does not contradict, the existing boundary rule ("process
definitions cross the boundary as source text, in exactly one place" —
process-channels spec): the source text is now the expansion residue.

## Rule 2: Pure expander sandbox

Expander bodies (and anything they call) may use only an **expansion-safe
whitelist**: pure list/dict/string/math natives (`first`, `rest`, `nth`,
`cons`, `append`, `list`, `dict`, `get`, `str`, `source`, arithmetic,
comparisons) and other module functions that themselves stay inside the
whitelist. Forbidden: the mutation surface, `send`/channels, widgets, host
handles, `rand`, file/load natives — anything stateful or nondeterministic.

Enforcement reuses the publish-time whitelist seam already specced for shipped
process bodies (process-channels spec, "Scheduler-safe native set"): violations
surface as compile diagnostics naming the offending native and the macro. The
VM keeps a backstop check when the expander actually runs.

Purity is what makes Rule 1 sound: same source → same expansion, always, so
hot-swap identity (diff by source, preserve `:state` by name) keeps working and
a re-eval never produces spurious diffs.

## Rule 3: Deterministic gensym

Generated bindings need collision safety (`sig`'s `phase` and `h` are textbook
capture hazards) without breaking re-eval stability. A fresh symbol per
expansion would make every re-eval textually unique, defeating source-identity
diffing. Instead, `(gensym base)` inside an expander derives its suffix from
the expansion site identity — (buffer, ordinal within the buffer's evaluation,
optional explicit `:key`), the same convention widget keys and process IDs
already use — plus a counter within that expansion. Same site, same expansion →
same symbols; two sites never collide.

Full hygiene (Scheme-style) is explicitly out of scope; this is the CL/Clojure
model with the footgun filed down.

## Source mapping

Errors in expansion residue must point at the macro call, not the generated
code. Minimum bar: every diagnostic raised while compiling or running residue
carries "expanded from `<macro>` at `<buffer>:<offset>`", using the same
revision/byte-offset plumbing the inline-widget bindings already thread
(`:__source-revision`/`:__source-start-byte`). Pretty-printing the residue on
demand (a `macroexpand` native for the scratch buffer) is part of this slice —
it is also the debugging story (`(macroexpand '(sig "hello" …))`).

## Migration

- **alez.sig**: delete `sig-register`'s string building and `eval`; `rewrite`,
  `thread-form`, and the option parsers become expansion-time calls; the macro
  returns the `def-process` form directly. The `now-beats` runtime native is
  untouched.
- **alez.jaki**: `jak` returns a `def-sequencer` form whose tick body is the
  already-walked expansion; `:tick-source` (the string parameter) remains for
  compatibility but new code should not need it. `pat` expands authoring-side.
- The rejected cheap alternative — adding `:run-source` to `def-process` to
  mirror `:tick-source` — is superseded by this spec; do not add new
  source-string parameters once expand-before-ship lands.

## Deliberately excluded

- Scheme/Racket-style full hygiene and syntax objects.
- Expansion on the scheduler VM, or macros in shipped source.
- Reader macros / new surface syntax.
- Expansion-time access to runtime state (channel values, engine state) — the
  sanctioned pattern for value-dependent behavior remains inlets and channels.

## Open questions

- Whether `expand_macros` should memoize expansions per (macro, args) within a
  compile pass, or rely on purity making re-expansion cheap.
- Whether the expansion-safe whitelist should admit a read-only `source`-style
  introspection of module constants (leaning yes: constants are part of the
  program text, not runtime state).
- Interaction with the module `override` mechanism (content-tiers spec §6.1):
  overriding a macro must invalidate compiled callers or be restricted to
  pre-first-use.
- Whether the capture site should auto-declare value channels by scanning
  residue for `(chan name initial)` forms. Channel-widget walks (jak's
  `channel-walk`) still exist as expansion-time library code — they can
  `macroexpand-all` first and emit declaration/binding calls as generated code
  to stay pure — but residue is self-describing, so kernel-side auto-declare
  would shrink every DSL's walk to just widget-byte-range → binding mapping.
