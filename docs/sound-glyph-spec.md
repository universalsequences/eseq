# Sound Glyph Spec

A generative "plant" glyph that visualizes a **sound** (a param-set fork of a
track's shared instrument + fx chain) inside the sound-selector modal
(docs/modal-widget-spec.md §4) — and eventually anywhere a sound needs a face
(palette, lineage views, interpolation UIs).

Design conclusion from brainstorm (2026-07-31): three independent visual
layers, built in order, each shippable without the next.

| layer | driven by | changes when | phase |
|---|---|---|---|
| **Topology** (branch structure) | authored dgenlisp of the instrument/chain | instrument swapped | P1 |
| **Geometry** (lengths, angles, curl, node sizes) | the sound's normalized param values | params edited | P2 |
| **Color** (gray = shared, sound-color = diverged) | diff vs. a baseline sound | baseline/context changes | P3 |

Key invariants:

- **Shape is identity.** Topology + geometry are a pure function of
  (instrument source, param vector). Same sound → pixel-identical glyph,
  everywhere, every frame. Any organic irregularity is seeded from a hash of
  the node path — never `random`, never iteration order.
- **Color is context.** Within one selector, every sound shares the track's
  chain, so topology is constant across boxes; identity comes from geometry,
  and the diff coloring is relative to *the clip's currently linked sound*
  (see §5). The current sound's own glyph is therefore all-gray — a
  self-confirming property.
- The interpolation UI ("morph sound 2 → 3 with a slider") falls out for
  free: lerp the param vectors, the geometry function morphs the plant. No
  extra machinery; deferred but protected by the purity invariant.

## 1. Skeleton extraction (topology)

Reality of the source (verified against `instruments/core/operator/dsp.lisp`):
custom-instrument `dsp.lisp` bodies are **flat** — a block of `(param …)`
declarations, then a chain of top-level `(def …)` forms ending at `out`. The
tree is not syntactic nesting; it is the **dataflow DAG** (def → referenced
defs/params). Extraction therefore works on the *pre-expansion authored
source* (never the expanded DSP graph, which is thousands of nodes):

1. Parse `dsp.lisp`; collect `param` declarations and top-level `def`s /
   `defun`s / `make-history` forms.
2. **Cluster params by name prefix** (`opa_*`, `lfo_*`, `filter_*`, `fenv_*`,
   `shaper_*`, …): longest shared `snake_case` prefix with ≥2 members;
   singletons (e.g. `tone`, `transpose`) fold into a `global` cluster. A
   sub-prefix cluster folds into its parent cluster when one exists
   (`lfo_to_*` joins `lfo`); sub-groups only stand alone when no parent
   cluster formed (`env_loop_*` / `env_sync_*` with no bare `env_*` params).
   This matches how authors already group params (declaration blocks,
   ui.lisp sections) and lands naturally at ~8–30 clusters.
3. Build the def-reference graph; assign each def to the cluster(s) whose
   params (transitively) feed it; collapse linear chains. Branch order =
   source order of each cluster's first param (stable across edits that don't
   reorder declarations).
4. Result: `Skeleton { branches: Vec<Branch { cluster, weight, children }> }`
   where `weight` ≈ number of params + defs in the cluster (drives visual
   heft). Cap rendering at ~30 branches; overflow merges smallest clusters
   (log nothing, but keep the merge deterministic: smallest-first, then name).

Cache per instrument source hash; recompute only when the instrument changes.

### Builtins (no lisp source)

Sampler and builtin effects (Roar, Space Echo, OTT, Filterbank, …) get
**hand-authored stock skeletons** — a small canonical branch list per builtin,
same `Skeleton` type, still param-grouped so geometry + diff layers work
unchanged. v1 fallback: one generic radial skeleton with branches from the
builtin's param groups. (Character pass later — e.g. Space Echo's three heads
as three fronds.)

### Chain composition

A sound covers the whole track chain (midi fx → instrument → fx). Render as a
single plant: stem segments per device in chain order (midi fx at the root,
instrument as the main canopy, fx above), each device contributing its
skeleton's branches off its stem segment.

## 2. Geometry (per-sound identity)

Each branch's rendered geometry is driven by its cluster's params, normalized
to 0–1 via `@min`/`@max` from the param declarations:

- branch **length/thickness** ← cluster weight + level/amount-ish magnitudes
- branch **angle/curl** ← remaining normalized values folded via fixed
  per-branch hash weights (deterministic, path-seeded)
- **node marks** along a branch ← individual params, sized by normalized value

The exact mapping is expected to be iterated by eye via capture scripts (§6);
the spec constraint is only purity + normalization, not the aesthetics.

## 3. Diff layer (P3)

- **Baseline = the clip's currently linked sound.** The selector's question is
  "what changes if I swap," so resting-state coloring answers it. (Hover-to-
  compare re-baselining: deferred.)
- Per-param delta in normalized 0–1 space; epsilon threshold (~0.02) so
  cutoff 200 vs 201 stays gray. Aggregate per branch (sum of deltas of the
  cluster's params); tint the branch in the sound's timeline color with
  intensity ∝ aggregate, plus a faded trail toward the root so deep diffs
  survive thumbnail size.
- Different-preset / different-everything sounds saturate fully — fine; the
  preset badge (§4) explains why.
- **Sort the selector grid by aggregate divergence** (nearest siblings first):
  turns the diff data into navigation.

## 4. Selector box anatomy

- top-left: sound name/number + timeline color chip (same as clip dots)
- center: glyph region (placeholder → plant, §6 phases)
- bottom-right: preset-name badge, `*` suffix when diverged from the preset
- current sound's box: distinct border + "linked" tag
- click = select/preview; explicit action (button / double-click) = link —
  relinking is the consequential act, keep it deliberate.

## 5. Where the diff is computed

Not in the UI layer. Precompute per (sound, baseline) pair — normalized param
vectors compared once, memoized, invalidated on param edit — and feed the
widget a ready per-branch divergence vector alongside the skeleton + values.

## 6. Phases

- **P0 — placeholder.** Ship the sound-selector modal (modal-widget-spec
  phase 4) with a trivial glyph region: per-device divergence bars or just
  name/chip/badge. Unblocks the modal payoff; everything below is a visual
  track of work behind the same box contract.
- **P1 — skeleton extraction lib.** Pure Rust: source → `Skeleton` +
  param→branch map. No UI. Unit tests against real instruments (operator,
  wavetable, triton): cluster counts, stability under whitespace/comment
  edits, determinism (two runs byte-identical).
- **P2 — monochrome glyph widget.** New widget (register in
  `BUILTIN_WIDGET_NAMES` — see phaser-flanger gotcha), fed skeleton + values;
  geometry mapping iterated via `metal_seq capture` scripts with hardcoded
  sounds. No color, no diff. Landing the raw shape is the iterative part —
  keep this phase open-ended.
- **P3 — diff coloring + grid sort.**
- **Deferred:** interpolation slider, fork-lineage tree view,
  hover-to-compare, per-builtin character skeletons, watch-plant-grow-on-edit.

## 7. Testing

- Extraction: pure unit tests, `-p sequencer` scoped (never package-wide).
- Widget layout: ui-script pattern, children via `each` never `map`.
- Visuals: checked-in capture scripts per instrument for eyeballing geometry
  iterations (same workflow as modal phase-0 capture support).
