# Param Namespacing — groups as resolvable namespaces

Rev 1 · 2026-08-26 · design only, not scheduled. Motivated by multi-operator
patches (FM `op1`/`op2`/…) in the factory-macro-library effort
(docs/factory-macro-library-spec.md, eseq-i2pw).

## 1. Problem

`@group` exists but is display-only. Declaring the same short name in two
groups —

```lisp
(param attack @group op1)
(param attack @group op2)   ; collides: params are flat bindings
```

— fails, so multi-operator instruments can't reuse natural names. The
workaround (`param op2.attack @group op2`) works mechanically (dots are
legal in param atoms) but the UI then shows the verbose full name inside a
section that already says `op2`.

Wanted: `[adsr op1.attack op1.decay …]`, `[* op2.mod_index~]`, and knobs
that show just `attack` inside the `op2` section.

## 2. Design: `@group` becomes a namespace

**Declaration is unchanged.** `(param attack @group op2 …)` — short name +
group, exactly today's syntax. No dotted declarations needed (the §1
workaround becomes obsolete).

**Uniqueness key becomes `(group, name)`** instead of `name`. Two `attack`s
in different groups are legal; two in the same group (or two ungrouped) are
still an error.

**Reference resolution** (dgenlisp symbol lookup, applies uniformly in
source text, patcher node text, and `(mod …)`):

1. A dotted symbol `op2.attack` resolves exactly to the param with
   group `op2`, name `attack`. Error if absent.
2. A bare symbol `attack` resolves iff exactly one param (any group) has
   that short name — full back-compat, since all existing content has
   unique short names. Ambiguous bare reference = compile error listing the
   dotted candidates.
3. Sugar composes: `op2.mod_index~` and `(mod op2.mod_index)` accept the
   dotted form (the dot is part of the symbol, left of `~`).

**UI:** knob label = short name; section = group. This falls out for free
once declarations keep short names. Migration courtesy: if a param's name
literally starts with `"<group>."`, strip that prefix for the knob label —
makes existing dotted-name workaround content look right immediately, even
before the resolver ships.

## 3. Identity: the dangerous seam

Host-facing param identity (p-locks, macro mappings, saved projects,
patch-learn, param_index) must not silently change. History says this bites
hard (plock-identity + instrument-fork param_index remap incidents).

- **Canonical host-facing id**: `group.name` when `@group` is present,
  bare `name` otherwise. Manifests, ParamTarget, serialization all use the
  canonical id going forward.
- **Load-time alias**: anything referencing a bare name (existing projects,
  p-locks, macro maps) resolves by the same rule as §2.2 — unique short
  name wins. Existing content has unique short names, so migration is
  lossless; log + drop on genuine ambiguity (only reachable in new
  content).
- Params **without** `@group` keep their exact current identity — zero
  change for the whole existing catalog.
- Audit checklist while implementing: ParamNodeId construction, p-lock
  save-back masks, macro-mapping ParamTarget, patch-learn plans, preset
  files, `@generated-for` links, take/sound-binding param references.

## 4. Where the work lives

1. **dgenlisp (vendored, ~/code/swift/dgen)**: `(group, name)` uniqueness;
   dotted symbol resolution in the evaluator; ambiguity errors; dotted
   forms through modulation lowering (`~` sugar, `@mod`). Manifest emits
   canonical ids + short display names. Needs revendor via
   content/dgenlisp.lock.
2. **eseq host**: dgen_manifest.rs canonical ids; load-time bare-name
   aliasing; identity audit (§3).
3. **Patcher**: dotted symbols in node/editor text (`[adsr op1.attack …]`,
   `[* op2.mod_index~]`) — verify the lexer treats `.` as symbol-interior
   (the bracket-attribute lexer bug is precedent for this class); param
   node display labels show the short name; retype/writeback round-trip.
4. **UI**: knob label shortening (+ the `"<group>."` strip courtesy);
   confirm `@env`/`@role` envelope-identity grouping is keyed by canonical
   id, not display name.

## 5. Non-goals / future

- No nested namespaces (`op1.env.attack`) — one dot, group-deep, matching
  the one-level `@group` model.
- No param templates yet. The real prize for multi-operator instruments is
  a macro that *instantiates* a param group per operator (declare the op
  param set once, stamp out `op1.*`…`op4.*`). Blocked on this spec and on
  the mod-in-macro constraint; noted in factory-macro-library spec §4.
- Non-param bindings (`def`, tensors) stay flat; namespacing is a param
  feature.
