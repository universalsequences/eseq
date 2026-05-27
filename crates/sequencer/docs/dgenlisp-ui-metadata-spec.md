# DGenLisp UI Metadata Spec

## Purpose

Custom DGenLisp instruments and effects without a hand-written `ui.lisp`
currently fall back to a generic Synth tab — a flat list of label / number-picker
/ slider rows. The result looks visually unrefined next to instruments that
ship a `ui.lisp`, even when the underlying DSP is well structured.

This spec adds a small set of **semantic attributes** to DGenLisp params so
DSP authors can declare *what params belong together* and *which params form
an envelope*. A deterministic layout generator in the host consumes that
metadata via the compiled manifest to produce a refined default UI — panels
per group, dedicated ADSR widgets for envelope role-sets — using the existing
eseqlisp layout primitives.

The goal: every instrument that opts in to the metadata gets a UI that looks
intentional, without an agent in the loop and without anyone writing a line of
eseqlisp.

## Design Goals

- **Capture structure where it's already known.** The DSP author already
  understands "these four params are an ADSR for operator 1" at the moment
  they declare them. Encode that there, once.
- **Strictly additive.** Instruments that don't use the new attributes keep
  today's flat default UI. Adoption is per-instrument and incremental.
- **Deterministic.** Given the same metadata, the layout generator produces
  the same UI. No agent, no fuzzy matching against param names.
- **Reuse the existing layout system.** The generator emits the same
  primitives (`h-stack`, `v-stack`, `box`, ADSR envelope widget, etc.) a
  hand-written `ui.lisp` would use.
- **Patch-editor friendly.** No new top-level node types to invent — v0.1 is
  purely additional attributes on the existing `param` node. Patching is the
  same flow with three extra optional attribute slots.
- **No lisp for casual users.** Authors write DGenLisp DSP code; the UI falls
  out of the attributes.

## Non-Goals (v0.1)

- **No new top-level forms.** No `defgroup`, no `defenv`. Groups and
  envelopes are materialized implicitly from param attributes. Richer
  group/envelope metadata (custom labels, color, ordering, collapse state)
  can later live in a separate lisp-driven sidecar with optional GUI — this
  spec stays focused on the minimum data layer needed to drive layout.
- No visual UI builder / free-arrange editor.
- No `@repeat-of` / repeated-group hint. Sibling layout is inferred from
  structural similarity (groups with identical param/envelope shape are laid
  out uniformly). Revisit if real ambiguity shows up.
- No `@target` on envelopes. The ADSR widget renders from A/D/S/R values
  alone; coupling it to a modulation target is deferred until there's a
  concrete visual reason.
- No replacement for hand-written `ui.lisp`. That path remains as the override
  for instruments with genuinely irregular layouts.
- No retroactive migration of existing custom-UI instruments.

## New Attributes on `param`

Three attributes are added to the existing `(param ...)` form (see
`DGenLispReadme.md` for current attributes like `@default`, `@min`, `@unit`,
`@mod`).

| Attribute | Value | Meaning |
|-----------|-------|---------|
| `@group`  | group name (symbol) | Place this param inside the named group's panel. The group is materialized implicitly from the set of params that reference it. |
| `@env`    | env name (symbol)   | This param is part of the named envelope. The envelope is materialized implicitly from the set of params that reference it. |
| `@role`   | `attack` \| `decay` \| `sustain` \| `release` | Which slot of the envelope this param fills. Required when `@env` is set. |

`@env` implies the param is *consumed* by the ADSR widget rather than rendered
as a standalone knob.

A param may carry both `@group` and `@env` — `@env` places the param inside
an envelope role-slot; `@group` (on any of the envelope's params) places the
resulting ADSR widget inside that panel. All params sharing the same `@env`
must agree on `@group`, or it's a validation error.

### Implicit Groups and Envelopes

- A **group** comes into being the first time any param uses `@group <name>`.
  Its label defaults to a humanized form of the symbol (`op1` → "Op 1",
  `amp` → "Amp"). No declaration is needed or possible in v0.1.
- An **envelope** comes into being the first time any param uses `@env <name>`.
  It's expected to collect exactly four params with distinct `@role` values
  (`attack`, `decay`, `sustain`, `release`). Missing roles are a validation
  warning; the envelope falls back to rendering its present params as
  individual knobs. Extras are an error.
- An envelope's group is whatever `@group` its constituent params share. If
  none of its params carry `@group`, the ADSR widget renders at the top
  level alongside other ungrouped controls.

## End-to-End Example (Digitone-style FM)

```lisp
(param op1-ratio   @group op1 @default 1.0 @min 0.25 @max 16)
(param op1-level   @group op1 @default 0.8 @min 0    @max 1)
(param op1-attack  @group op1 @env op1-env @role attack   @default 0.01)
(param op1-decay   @group op1 @env op1-env @role decay    @default 0.2)
(param op1-sustain @group op1 @env op1-env @role sustain  @default 0.5)
(param op1-release @group op1 @env op1-env @role release  @default 0.3)

(param op2-ratio   @group op2 @default 2.0 @min 0.25 @max 16)
(param op2-level   @group op2 @default 0.4 @min 0    @max 1)
(param op2-attack  @group op2 @env op2-env @role attack   @default 0.01)
(param op2-decay   @group op2 @env op2-env @role decay    @default 0.4)
(param op2-sustain @group op2 @env op2-env @role sustain  @default 0.3)
(param op2-release @group op2 @env op2-env @role release  @default 0.5)
```

The layout generator sees:

- Two implicit groups, `op1` and `op2`, with default labels "Op 1" and "Op 2".
- Two implicit envelopes, `op1-env` and `op2-env`, each with all four roles
  filled, each bound to its respective group.
- Each group contains the same shape: two knobs (`ratio`, `level`) plus one
  envelope.
- Structural similarity ⇒ render `op1` and `op2` as sibling panels in a
  uniform row/grid.
- Within each panel: knobs grouped, ADSR widget rendered once for the
  envelope.

Params with neither `@group` nor `@env` continue to render in the default
flat list.

## Layout Generator Behavior

Given the metadata graph, the generator picks a preset:

| Condition | Preset |
|-----------|--------|
| No `@group`/`@env` metadata anywhere | Current flat default UI (no change). |
| N ≥ 2 implicit groups with identical shape | Uniform sibling grid/row of panels. |
| Single group, or groups with differing shapes | Stacked panels, ordered by first-reference order. |
| Implicit envelope with full role-set | ADSR widget rendered; envelope's four params consumed. |
| Implicit envelope with missing roles | Validation warning; fall back to individual knobs for present roles. |
| Params with the same `@env` but conflicting `@group` | Validation error at compile time. |

"Identical shape" is structural: same set of non-envelope param positions
(modulo group-prefix differences in their names), same envelope presence.
Tunable later if false positives/negatives show up.

## Manifest Changes

The compiled JSON manifest (see `dgenlisp-api.json`) gains:

- Each param entry gains optional `group`, `env`, and `role` fields.
- `groups`: derived array of `{ name }` — the set of distinct group names
  referenced by any param, in first-reference order. Labels are *not* in the
  manifest in v0.1 (host humanizes the name).
- `envelopes`: derived array of `{ name, group, roles: { attack, decay, sustain, release } }`
  where each role value is the corresponding param name (or null for missing
  roles), and `group` is the common `@group` of the envelope's params (or
  null).

The runtime layout generator (in the host, not in DGenLisp itself) reads the
manifest and produces the eseqlisp UI tree. DGenLisp's job ends at parsing
attributes, validating them, and emitting the manifest; it does not generate
UI lisp.

## Compiler vs. Host Split

- **Compiler (DGenLisp):** parse `@group`/`@env`/`@role`; validate
  (missing/extra envelope roles, conflicting `@group` within an envelope,
  `@role` without `@env`); emit derived `groups`/`envelopes` arrays into the
  manifest.
- **Host (this repo):** read manifest; run structural-similarity pass; pick
  a layout preset; emit the eseqlisp UI tree using existing primitives.

## Future Metadata Layer (Out of Scope for v0.1)

Richer group/envelope-level metadata — custom labels, colors, collapse state,
ordering hints, layout-preset overrides — can later live in a separate
lisp-driven sidecar file (e.g. `ui-meta.lisp`) that the host merges with the
manifest before laying out. That sidecar is a natural target for a small GUI
("rename group `op1` to 'Operator 1', set its color to blue"), avoiding the
need to either invent new DGenLisp node types or hand-write eseqlisp.

The v0.1 data layer in this spec is intentionally sufficient on its own — the
sidecar is an enhancement, not a prerequisite.

## Relationship to Existing Custom UI (`ui.lisp`)

- If an instrument ships a `ui.lisp`, it wins. The metadata is ignored for
  layout purposes (though it may still be useful for the agent-mode e2e flow,
  for modulation manifests, etc.).
- If `ui.lisp` is absent and metadata is present, the generator runs.
- If both are absent, today's flat default UI renders.

This lets the metadata-driven UI serve as a strong default while preserving
`ui.lisp` as the escape hatch for irregular layouts.

## Relationship to Agent-Mode E2E Build

The "build instrument e2e" agent currently picks a layout preset (packed,
sparse, etc.) and emits `ui.lisp`. With this spec in place, the agent's
preferred output shifts: instead of generating `ui.lisp`, it generates DSP
with rich `@group`/`@env`/`@role` attributes and lets the deterministic
generator produce the UI. The agent only emits a custom `ui.lisp` when the
instrument genuinely needs a layout the generator can't express.

This narrows the agent's surface area and makes the result reproducible.

## Open Questions

- Should params with no `@group` render above or below grouped panels? Lean
  toward below, as a "misc" section.
- Do we need a way to mark a param as `@hidden` from the generated UI but
  still present in the manifest (for modulation-only params)? Likely yes;
  add `@hidden true` as a follow-up if not already supported.
- Humanization rules for group/env names: just title-case with separator
  replacement (`op1` → "Op 1", `amp_env` → "Amp Env"), or something smarter?
  Start simple.
- When/whether to reintroduce explicit `defgroup`/`defenv`, `@repeat-of`,
  `@target` — only if a concrete case demands them, and likely as part of
  the future sidecar layer rather than DGenLisp itself.
