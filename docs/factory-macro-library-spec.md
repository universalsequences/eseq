# Factory Macro Library — showcase instruments as readable patches

Rev 1 · 2026-08-26 · feeds eseq-2k9p.2 (showcase curation / shipped-content
manifest) and eseq-4tr.2 (release bundle).

## 1. Motivation

Every current custom instrument is a oneshotted LLM-authored dgenlisp blob.
They work, but:

- Opening one in the patch editor is indecipherable — a spaghetti of wires.
  Factory content actively demonstrates that the editor is unusable on real
  patches, which is backwards for a product whose pitch includes patching.
- They cannot be iterated by a human, only regenerated. "Drift is muddy" or
  "morph1 is too heavy" are hard to fix because there is no shared structure
  to fix them *in*.
- Users can't learn from them or fork them meaningfully.

The v0.1 showcase set is greenfield, so we take a different approach: build
the factory instruments from a small **shared defmacro vocabulary** shipped
in the factory content tier. The top level of each instrument is then a
handful of macro nodes wired together — readable, forkable, and demonstrating
the intended authoring style. The macro library is itself content
(`content/defmacros/…`, factory tier per the content-tiers spec), so users
extend the same language the factory instruments speak, and the
customize→extend→override→shadow ladder applies to instrument internals.

Secondary win: the author can iterate on factory sound design by patching
instead of prompting.

## 2. Goals

1. 5–10 showcase instruments whose dsp is a composition of factory macros;
   opening any of them in the patch editor is legible at the top level **and**
   one level down (inside each macro).
2. A factory macro vocabulary small enough to defend as a stable API
   (target: ~8–15 macros at v0.1, not 30).
3. Shared tuning: fixing the saturation stage, filter, or sine approximation
   in one macro improves every instrument that uses it.
4. A shipped-content manifest that includes the macro dependency closure.

## 3. Non-goals

- Runtime code sharing / CPU savings from the macros themselves. Macros
  expand at compile time; perf gains come from the pruning and tuning the
  shared structure enables (e.g. one lean SVF instead of morph1's topology
  zoo), not from sharing.
- Rewriting instruments that don't make the showcase cut. Non-shipping
  content stays in the repo as-is.
- A general package system. The macro library should land in whatever
  namespacing exists at v0.1 (see §5), but this spec does not design
  packages.

## 4. Method: extract the vocabulary bottom-up

Do **not** design the library first. The sequence is:

1. Rebuild 2–3 instruments in the patch editor (suggested order: wavetable,
   stripped-down analog-bread-and-butter, FM). Author freely.
2. Where real repetition appears across them, extract macros — Cmd+E
   encapsulate is the tool. Promote a macro to the factory library only when
   a second instrument wants it.
3. Repeat for the rest of the roster. Expect the vocabulary to converge
   around: oscillator/voice stack (unison, detune, drift), one SVF + one
   ladder filter, ADSR/VCA idioms, a tunable drive/saturation stage, an FM
   `op`, and small math utilities (`fast-sine`, db↔gain, semi↔ratio).
4. Prune the existing `content/defmacros` scratch (`simp2`, `simp10`,
   `simp11`, `gain2`, …) — the shipped library is greenfield; keep only
   entries that earn a place (e.g. `unison-frequencies`,
   `schroeder-allpass` if used).

### Macro design constraints (hard-won, do not violate)

- **`(mod param)` cannot appear inside a macro body** — host-modulated params
  must be resolved at top level and passed into macros as already-resolved
  signals (see the pre-resolve block in `content/instruments/core/wavetable/
  dsp.lisp` and the operator-fm postmortem). Corollary: factory macros take
  *signals*, never param names. `(mod X)` also requires a bare top-level
  param (patcher mod-sugar bug notes).
- Macros must round-trip through the patcher: display labels, bracket
  attribute arrays (`@shape`/`@data`), and nested library-macro imports all
  have fixed bugs behind them — any remaining round-trip lossiness found
  during this work is a blocker for this spec, not a nice-to-have.

## 5. Namespacing and API stability

The moment v0.1 ships, factory macro names and arities are a public API:
user forks reference them, and changing a macro changes every fork. Rules:

- Ship the vocabulary in a dedicated namespace/prefix from day one (a
  factory package if packages land in time — natural first customer for the
  packages epic — otherwise a naming convention like `fct-*` that can be
  aliased into a package later with the warn+migrate pattern).
- Post-v0.1 changes to a shipped macro are either (a) behavior-preserving
  tuning, (b) additive optional args with defaults, or (c) a new macro name.
  The module-spec §6.1 `override` mechanism is the user-side escape hatch,
  not a license for us to break signatures.
- Keep it small. Every macro added is a forever-name.

## 6. Interpretability requirements

"User can look inside" is recursive — it fails if the top level is clean but
double-clicking a macro shows spaghetti.

- Every factory macro ships with a hand-authored `macro.layout.json` sidecar,
  reviewed by actually opening it. The auto-layout row compaction helps but
  is not a substitute.
- Every showcase instrument's top-level patch layout is hand-arranged and
  saved. Target: top level fits on one screen; signal flow reads left→right
  or top→bottom.
- Macro display names are user-facing vocabulary ("SVF Filter", "Unison
  Osc"), not identifiers.

## 7. Asset-referencing macros (the wavetable problem)

Today the wavetable bank is `(def bank (tensor @shape [512 448] @file
"waves/bank.json"))` — a compile-time file reference frozen into the dylib.
The patcher *already round-trips* tensor/tensor-param/audio-tensor `@file`
attributes (writeback-tested); what's missing is UX and runtime swap. The
design assessment in **bead eseq-26u** applies wholesale and this spec adopts
its model rather than inventing a new one:

- **Factory instruments (v0.1 scope):** compile-time `@file` tensors are
  fine — the bank is part of the instrument. The macro-library requirement is
  only that the tensor node round-trips and displays sanely in the patcher
  (it does), and that a `wavetable-osc` macro takes the bank tensor as an
  input rather than baking the path inside the macro, so a fork can point at
  a different bank by editing one node.
- **User-swappable tables (post-v0.1, tracked in eseq-26u):** tensor-param +
  host-side asset binding (`@asset "stem"` scanned like `asset_references()`,
  resolved in Rust, written via `queue_tensor_write`), plus patcher asset-node
  sugar with a stem dropdown and browser drag-drop. Do not use compile-time
  `@file` for anything user-swappable.
- Manifest consequence: an instrument's shipped closure includes its `@file`
  assets (§9).

## 8. Exemplar macro: `fast-sine`

First concrete library entry, and the template for "shared tuning" wins: a
degree-13 root-factored Chebyshev sine (moooo.ooo derivation; max abs error
≈5.9e-8), input −0.5..0.5 = one cycle:

```lisp
(defmacro fast-sine (signed_phase)
  (let ((p2 (* signed_phase signed_phase)))
    (* signed_phase
       (- p2 0.25)
       (+ (* (+ (* (+ (* (+ (* (+ (* 3.1616015434265137 p2)
                                  -14.049662590026855) p2)
                            38.49587249755859) p2)
                      -67.07662200927734) p2)
                64.83582305908203) p2)
          -25.132740020751953))))
```

(Exact dgenlisp let/nesting syntax to be settled when built; the shape is a
Horner chain.) Feeds from `(- (phasor f trig) 0.5)` modulo a half-cycle
offset (negate or pre-wrap `(+ phase 0.5)`).

Status: **BUILT** (`content/defmacros/fast-sine/`, phasor-domain input 0..1
with an internal wrap so FM-style phase offsets are safe) and harness-
verified 2026-08-26 (scratch harness, not checked in):

- Emitted C is pure mul/add — no `vsinf`, no libm, and the wrap lowers to
  `x − floor(x)`. Max abs error vs double sin: 8.7e-6 (−101 dB; dominated
  by the phasor's float32 phase accumulation, not the polynomial).
- **Evaluation shape matters more than the polynomial.** The first (Horner)
  version lost badly in a per-sample feedback-FM patch — 36.3 vs 21.8
  ns/sample against builtin `(sin (* 2π ph))` — because history feedback
  makes the loop latency-bound and a Horner chain is a fully serial
  dependency chain, while Apple's libm `sinf` has a shorter critical path.
  Rewriting as Estrin's scheme (parallel linear halves, ~half the
  dependency depth) reached parity in the feedback loop (22.4 vs 22.1) and
  is what the library ships.
- Measured (M-series, 512-frame blocks): SIMD path `phasor → sine → out`
  5.23 vs 5.5 ns/sample (builtin `vsinf` pays Cody-Waite radian reduction;
  the macro takes cycle-domain phase). Scalar feedback-FM: parity.
- Net: the macro is never slower, slightly faster when vectorized, and
  patcher-legible — but the perf story is legibility + cycle-domain
  convenience, not a big speedup. Lesson for future math macros: in
  feedback (scalar, latency-bound) contexts, prefer shallow dependency
  trees (Estrin) over Horner.
- Upstream option remains: a cycle-domain `sin1` op or scalar polynomial
  in the toolchain would benefit all content at once.

## 9. Shipped-content manifest

The manifest for eseq-2k9p.2 / eseq-4tr.2 is no longer a list of instrument
folders. It is:

- the showcase instrument folders (dsp.lisp, ui.lisp, presets, `@file`
  assets like `waves/bank.json`), plus
- the **transitive factory-macro closure** they reference
  (`content/defmacros/<name>/…` including layout sidecars).

The bundle build must resolve this closure mechanically (scan macro
references, or a checked manifest that a test validates against actual
references) — a hand-maintained list will silently break the first time an
instrument gains a macro dependency.

## 10. Roster (working)

| Slot | Base | Work |
|---|---|---|
| Wavetable | core/wavetable | rebuild as macros; steal ideas from digipro/monomachine wavetables |
| Analog B&B | core/analog-bread-and-butter | strip down ~2x perf; donates voice/filter/env macros |
| FM | greenfield (not the operator/digitone clones) | `op` macro; algorithm = visible wiring. Strongest demo of the approach |
| Acid | existing tb303 | generalize |
| Drift | core/drift | keep only if the shared drive stage fixes the mud |
| ~~Morph1~~ | — | cut from v0.1; its topology zoo becomes "one SVF + one ladder macro"; can return later as just-a-patch |

5–10 total; drums/percussion slots TBD against eseq-2k9p.1 (sample kits).

## 11. Risks

- Patcher round-trip lossiness (display labels, paste) resurfacing at scale —
  treat as blockers.
- Vocabulary creep: >15 macros at v0.1 means we shipped an API we haven't
  earned. Cut instruments before growing the library.
- UI is a separate surface: a legible patch does not fix a bad panel
  (`ui.lisp`). The FM synth in particular needs UI work regardless.
- Perf regressions from "readable" structure: keep the C-harness benchmark
  loop in the workflow; readable and fast are both requirements.
