# Delta Glyph Spec

A parameter-difference glyph for the sound palette: a sparse SDF blob field on a fixed
lattice, where **occupancy encodes which parameters differ from a reference patch and by
how much**, normalized by how much each parameter actually varies across the cohort on
screen.

Companion document: [`sdf-blob-glyph-algorithm.md`](./sdf-blob-glyph-algorithm.md) —
the reverse-engineered `spores-next` renderer this borrows its rendering model from.
This spec reuses that renderer's *geometry and shading* (rounded-box SDFs, polynomial
smooth-union, height-field normals, layered compositing with creases) and replaces its
*content model* (name hashing → polyominoes) entirely.

Status: rev 3. Each revision was implemented and then corrected against what it
actually rendered; §0 records the history, and corrections are marked **[rev N]** in
place.

---

## 0. Revision history

**Rev 3** — after rev 2 rendered. Rev 2 fixed the normalization but produced a shape
vocabulary of exactly two forms: an isolated circle, and two diagonally-adjacent circles
in a soft peanut. Never three cells, never four, never the elongated stadiums. Re-reading
the original showed the cause was not tuning but **a wrong model of what the smooth union
is applied to**, plus two arithmetic errors of mine:

1. **The fusion threshold was wrong by 1.55×.** I wrote that bridging begins when the
   surface gap drops below `k`. Two equal discs each read `gap/2` at their midpoint and
   the smooth-min's maximum sag is `0.5·k/1.55 = 0.32258·k`, so they merge when
   **`gap ≤ 0.6452·k`**.
2. **Only horizontal adjacency was ever checked.** The lattice's vertical and diagonal
   neighbours sit at `D = √(0.18² + 0.3636²) = 0.4057`, and **11 of the original's 14
   piece types are built from vertical or diagonal offsets**. Welding those needs
   `R ≥ 0.1706`; rev 2's `R_MAX` was `0.16`. Vertical fusion was not rare, it was
   *arithmetically impossible at any deviation*. The "peanuts" on screen were never
   fused pairs — they were positive-valued necks painted by the coverage blur.
3. **The scene unit was wrong.** In the original one smooth-union scene is a
   **contiguous polyomino** of 1–5 cells, mean 3.29, with 36% of scenes at 4–5 cells.
   Rev 2 fed it whatever per-parameter cells happened to be lit, with no adjacency
   requirement at all — so most layers *could not* fuse regardless of radii.
4. **Radius must not carry magnitude.** In the original every cell is a disc of radius
   exactly 0.18 and radius never varies; all variation is *which cells are occupied*.
   "Radius encodes magnitude" and "cells weld into blobs" are in direct competition on
   this lattice, and rev 2 chose the side that forbids blobs. See §6.1.
5. **Capsules were a rare marker instead of a normal outcome.** The original emits one
   for 5 of 19 piece types (~26% of entries), always at full radius and always paired
   with a partner 0.0036 away. Rev 2 required a `link` + same group + adjacency, which
   almost never fires, and drew them at variable radius as thin worms.
6. **Layers could not interpenetrate.** The original anchors every piece into one shared
   grid, so adjacent layers overlap by up to 6 of 7 cells — ~22 cell-placements onto 16
   positions, 1.4× overcoverage, with intersection recolouring. Rev 2 encoded the layer
   as two bits *per slot*, so a slot belonged to exactly one layer and overlap was 0% by
   construction.
7. **The sign offset de-fused more than the fillet could close.** `±0.055·d` on two
   vertically-adjacent opposite-sign cells gives a gap of 0.187 — 2.9× the entire fusion
   budget. Sign moves off position entirely (§6.2).
8. **The identity substrate was over-corrected into nothing** (§5.1). Rev 2's uniform
   address grid fixed rev 1's noise problem by removing all information, so a cohort of
   similar patches read as a field of identical gray dots with no character.

Density, for scale: the original runs ~22 cell-placements over 16 positions; rev 2 ran
≤7 cells over 25 slots across three non-overlapping layers — roughly a fifth of the ink,
scattered rather than clustered.

**Rev 2** — after the first implementation. Rev 1 produced a palette in which five
near-identical patches rendered as bare constellations and one outlier rendered as a
single fused amoeba: all-or-nothing, no gradation. Six defects, five of them in the
spec:

1. **MAD spread collapsed** (§4.2). In a cohort of mostly-identical patches the median
   absolute deviation is ~0, so every parameter's spread hit `SPREAD_FLOOR` and the one
   differing patch divided by that floor and saturated everything. MAD measures how
   spread out the cohort's *center* is; the question is how much this patch differs *at
   most*. Replaced with a quantile of the deviations themselves.
2. **The radius range sat entirely inside the fusion zone** (§6.1). With `step = 0.3672`
   and `k = 0.155`, bridging begins at `r = 0.106`; rev 1's `R_MIN` was `0.11`, above
   it, so *every* adjacent lit pair fused unconditionally. `R_MAX = 0.21` also exceeded
   `step/2 = 0.1836`, so max-radius discs physically overlapped before the fillet
   applied. The "topological snap" was firing constantly instead of rarely.
3. **Per-glyph `k`** (§6.4) made glyphs mutually incomparable and compounded the mush.
   Cut entirely.
4. **No bound on total ink.** Nothing stopped a patch from lighting fifteen cells at
   once. Added a top-K cap (§4.6).
5. **The identity base layer was noise wearing the costume of data** (§5.1). Rev 1 drew
   it from absolute values at `R_BASE = 0.15`, producing a scatter of large beads that
   was near-identical on every tile, used the same shape vocabulary as the accents, and
   occupied most of the ink. Replaced with a faint uniform lattice dot.

6. **Selection-relative by default** (§7). Every tile changed shape on every click,
   which is disorienting precisely while scanning. The default is now a fixed **anchor**
   — the first patch in cohort order — so a glyph's shape is a property of the patch.
   This also makes cohort statistics selection-independent, removing the caching
   liability rev 1 carried.

Two further defects were in the implementation but are really spec omissions, now
stated explicitly in §3.4: **slot layout and layer color must be computed once per
cohort**, not per tile. Rev 1 dropped "dead" parameters using the *subject's* value and
picked accent hues from the *subject's* group totals, so different tiles in one palette
had different lattice sizes, different slot→parameter maps, and different color
meanings. Nothing is comparable under those conditions, which is the entire point of the
glyph.

**Rev 1** — original design.

---

## 1. Motivation and design constraints

### 1.1 What's wrong with the current plant glyph

The shipped sound glyph (see [`sound-glyph-spec.md`](./sound-glyph-spec.md)) draws a
skeleton extracted from the dgenlisp signal graph. In the sound palette that is the
wrong signal: **every patch in the palette is an instance of the same instrument, so
every patch has the same graph topology.** Ten patches that differ only in filter cutoff
produce ten glyphs that differ in one node's decoration, at thumbnail scale, by a few
pixels. The reader cannot tell them apart, and worse, cannot tell *how* they relate.

### 1.2 What the glyph must actually do

Ranked:

1. **Differentiate.** Two patches that differ audibly must look obviously different at
   ~120 px.
2. **Preserve the metric.** Two patches that are nearly identical must look nearly
   identical, and patches that differ in the *same way* must look the same way. The eye
   should be able to cluster a palette of 20 into families without reading labels.
3. **Localize.** The glyph should say *which* part of the patch differs (filter vs.
   envelope vs. FX), not merely *that* it differs.
4. **Stay a glyph.** It is a 120 px thumbnail with an aesthetic job, not a bar chart. Two
   or three colors, a coherent silhouette, no axes, no legend.

Requirements 1 and 2 are in tension only if the mapping is non-linear in the wrong
place. §4 resolves it: the mapping is monotone and continuous, and the *amplification*
comes from cohort-relative normalization plus one deliberate topological threshold.

### 1.3 Non-goals

- **Not a hash.** The spores algorithm hashes a filename into arbitrary polyominoes.
  Hashing is metric-destroying: it maximizes visual distance between similar inputs,
  which is the exact opposite of requirement 2. There is no hashing anywhere in this
  spec.
- **Not deterministic in isolation.** A delta glyph is a function of
  `(patch, reference, cohort)`. The same patch renders differently in a different
  palette. This is intentional and is the core idea. It is *not* a function of the
  selection, though — see §7.
- **Not a complete patch description.** It shows deviation only. A patch's absolute
  character is not encoded anywhere; the address grid (§5.1) is deliberately uniform.

---

## 2. Inputs

```
DeltaGlyphInput {
    subject:   PatchId          // the patch this glyph represents
    reference: PatchId          // anchor mode: the FIRST patch in cohort order (§7)
    cohort:    [PatchId]        // every patch visible in the palette, including both above
    schema:    ParamSchema      // per-instrument, shared by all of the above
}
```

**Cohort membership is the full palette contents, not the visible scroll window.**
Scrolling must not change any glyph. If the palette is filtered (by instrument, by
scene), the filter result is the cohort.

### 2.1 `ParamSchema` maps onto existing types

Nothing new needs to be authored — the schema is
`EffectDescriptor.params: Vec<ParamDescriptor>` (`crates/sequencer/src/effects/mod.rs:153`),
obtained per track via `app.graph.instrument_descriptors[track]`
(`crates/sequencer/src/app/mod.rs:553`).

| Spec field | Real source | Notes |
| --- | --- | --- |
| `id` | positional index `i` | `defaults[i]` is index-aligned with `params[i]` |
| `kind` | `ParamDescriptor.kind` | `Continuous{unit}` \| `Boolean` \| `Enum{labels}` |
| `range` | `.min`, `.max` | |
| `taper` | `.scaling` (`Linear` \| `Exponential`) | **effectively absent** — see §4.1 |
| `group` | `.ui_metadata.group: Option<String>` | free-form, instrument-authored |
| `order` | positional index `i` | source order from the manifest |
| `link` | `.ui_metadata.env` / `.role` / `.tags` | heuristic; see §6.3 |
| `weight` | — | not present; deferred (§12) |

The subject values are `patch.instrument_slot.defaults: Vec<f32>`
(`EffectSlotSnapshot`, `effects/mod.rs:8949`) — a positional `Vec<f32>`, **not** a
name-keyed map. There is no name→value mapping inside a `Patch` at all; names live only
on the descriptor. This is the same alignment the current glyph host already relies on
(`ui/state_values/sound_palette.rs:126-141`).

### 2.2 Schema compatibility is not guaranteed — filter the cohort

The descriptor available at render time is the **track's currently-loaded instrument**,
not the instrument each patch was authored against. A track's pool can hold patches from
a previous engine (`Patch.instrument_type`, `Patch.instrument_run_mode`), and the
existing glyph code has this same latent bug: it normalizes stale patches against the
wrong descriptor (`sound_palette.rs:126-141`, skeleton identity key at `:80-84`).

A delta glyph makes this worse, because one incompatible patch corrupts the *cohort
statistics* and therefore every other glyph in the palette. So it must be handled, not
inherited:

```
compatible(patch) = patch.instrument_slot.defaults.len() == descriptor.params.len()
                 && patch.instrument_type == track.instrument_type
```

Incompatible patches are **excluded from the cohort statistics entirely** and rendered
base-layer-only with a distinct marker (a hollow ring). They are honestly
un-comparable; pretending otherwise silently poisons the normalization.

---

## 3. The lattice

Geometry is inherited from the spores lattice
([algorithm doc §2](./sdf-blob-glyph-algorithm.md)) with the cell count parameterized.

```
CELL_HALF_EXTENT = 0.18          // base half-extent, uv units
CELL_RADIUS      = 0.18          // == half-extent ⇒ the primitive is a disc
SPACING_X        = 1.04          // multiplier on half-extent
SPACING_Y        = 1.02
STAGGER_X        = 1.0           // odd rows shift +0.18 (hex-ish packing)
STAGGER_Y        = 0.0

stepX = CELL_HALF_EXTENT * (1 + SPACING_X) = 0.3672
stepY = CELL_HALF_EXTENT * (1 + SPACING_Y) = 0.3636
```

Two changes from spores:

1. **Centered uv.** Use `uv = (2*fragCoord − resolution) / min(resolution.x,
   resolution.y)` — the spores mapping puts the origin in the corner and crops the
   lattice ([algorithm doc §1](./sdf-blob-glyph-algorithm.md)). Recenter the lattice on
   the origin so all cells are on screen.
2. **`COLS × ROWS` sized to the schema**, not fixed at 4×4. See §3.2.

### 3.1 Slot assignment

Slot index → parameter is **fixed per instrument** and must never change between
renders, sessions, or cohorts. A parameter that moves position between two glyphs
destroys comparability.

```
slots = schema.params
          .filter(visible)                    // §3.3
          .sorted_by(group_order, param.order, param.id)
```

then filled **column-major**: `slot = col * ROWS + row`. Consecutive slots are
vertically adjacent, and `slot + ROWS` is the horizontal neighbour — so a group forms a
contiguous spatial lobe, and both lattice adjacencies are expressible as slot
arithmetic. A group boundary landing mid-column is padded to the next column start when
the remainder is ≤ 1 cell, so lobes stay visually separable.

**[rev 3] Plain column-major, not boustrophedon.** Rev 1–2 reversed odd columns to avoid
the jump from the bottom of one column to the top of the next. That is fine when slots
are independent, but it breaks *horizontal* adjacency: under boustrophedon `slot + ROWS`
lands at a mirrored row, so it is not the neighbour to the right. Pieces (§6.1) are
defined by adjacency offsets and need both directions to be real, so the reversal has to
go.

This is the property that makes requirement 3 (localize) work: "the filter differs"
becomes a bright mass in a consistent region of the tile, learnable after about three
minutes of use.

### 3.2 Lattice size

```
n      = slots.len()
COLS   = ceil(sqrt(n * 1.15))       // slight landscape bias; palette cells are wider than tall
ROWS   = ceil(n / COLS)
```

**Clamped to `COLS, ROWS ≤ 5` (25 slots) at palette size.** Legibility is governed by
**fill ratio**, not cell count — a sparse lattice with five cells lit is a glyph, a dense
one is mush — but below ~20 px per cell the smooth-union fillets stop resolving.

Measured against the real tile: the palette modal is 960×800 px with a 2–4 column
`responsive-grid`, and the glyph is `:height 6.5` cells, insetting to the largest
centered square in pixels (`widget_render/sound_glyph.rs:112-126`). That lands around a
**90–110 px square**. At 6 columns each cell is ~16 px — under the floor. At 5 columns
it's ~20 px, right at it. So 5×5 is the palette cap; surfaces with a larger glyph may
raise it to 6×6, at the cost of slot layout no longer being identical across sizes
(§12).

If `n > 25`, the tail is aggregated: parameters beyond the 20th (by group order) are
collapsed into one **aggregate slot per group**, whose deviation is the RMS of its
members' deviations. Aggregate slots render with the capsule primitive (§6.3) to mark
them as composites. Aggregation is expected to be the common case, not the exception —
see §3.3.

### 3.3 Parameter visibility

Real dgenlisp instruments generate far more descriptor entries than are worth drawing.
`instrument_descriptor_from_manifest` (`lisp_host/dgen/dgen_manifest.rs:389`) appends,
after the authored params: tensor params, voice-modulator UI params, dgen modulator
descriptors, and synthesized `__dgen_mod_active__*` / `mod {dest} slot {n} amt` entries
(`dgen_manifest.rs:500-590`). Operator/Triton-class instruments run to well over 100
descriptor entries.

Excluded from slot assignment entirely:

- synthesized modulation plumbing: names matching `__dgen_mod_active__*` and
  `mod * slot * amt` (these are addressing metadata, not timbre)
- tensor params (`TensorParamSnapshot`) — wavetables, MSEG breakpoints. A meaningful
  delta needs a shape metric, not a scalar; deferred (§12). Optionally one aggregate
  slot per tensor carrying "changed / unchanged."
- `hidden` params — already filtered upstream by `from_lisp_manifest`
- parameters whose cohort spread is zero **and** whose value equals `descriptor.default`
  (dead params — they can never contribute and would waste lattice area)

Note the last rule's asymmetry: a parameter with zero cohort spread but a *non*-default
value is still assigned a slot. It contributes nothing to the delta field but it does
contribute to the base layer (§5.1), which is what encodes "this whole cohort is a
bright preset."

Even after exclusions, expect 40–80 slots on a real instrument, so the §3.2 aggregation
path is the primary path and must be good. Aggregate-by-group is the right default
because it degrades exactly into requirement 3 (localize): with heavy aggregation the
glyph still says *which subsystem* differs, just not which knob.

### 3.4 Layout and color are cohort-level, not per-tile **[rev 2]**

**Every tile in a palette must use an identical lattice: same `COLS × ROWS`, same
slot→parameter map, same aggregate chunking, same accent hue per group.** Comparability
is the product; a glyph whose slots mean something different from its neighbour's
communicates nothing.

Concretely, all of the following are computed **once per cohort** and shared:

- the visible-parameter set (§3.3) — in particular the "dead parameter" test must be
  evaluated over the **whole cohort**, never against one subject's values
- `COLS`, `ROWS`, boustrophedon assignment, group padding
- tail aggregation and its chunk boundaries (§3.2)
- linked-pair adjacency (§6.3)
- the two accent groups and their hues (§5.2) — ranked by each group's total deviation
  summed **across the cohort**, not within one patch

Only the deviation vector itself, and what follows from it (radii, offsets, the top-K
cut), varies per tile. The natural shape for this is a cohort object built once that
emits tiles: `Cohort::new(schema, patches, reference)` → `cohort.build(subject)`.

### 3.5 Viewport fit

Constants in §3 and §6 are in the spores uv units (half-extent 0.18, step 0.3672,
`k = 0.155`). For `COLS ≠ 4` the lattice must be scaled to fit the viewport:

```
extent_x = (COLS - 1) * stepX + 2 * R_MAX + MARGIN      // MARGIN = 0.08
extent_y = (ROWS - 1) * stepY + 2 * R_MAX + MARGIN
fit      = 2.0 / max(extent_x, extent_y)
```

Apply `fit` uniformly to all positions **and** radii **and** `k` **and** `blur` **and**
the crease erosion. Uniform scaling preserves every ratio the design depends on — most
importantly the bridging knee in §6.1 — so all constants stay expressible in the
unscaled units. Scaling `k` non-uniformly with the geometry is the one mistake that
would silently break the topological snap.

---

## 4. Normalization: patch → deviation vector

This is the core of the spec. Everything else is rendering.

### 4.1 Taper space

All arithmetic happens in **normalized taper space**, never in native units. `t` is
"where the knob is." A 200 Hz cutoff change is enormous at 300 Hz and inaudible at
8 kHz; differencing in native Hz makes the entire lower half of the filter range
invisible. Taper space is the closest cheap proxy for perceptual distance available
without a psychoacoustic model.

**The metadata for this is largely missing and must be inferred.** `ParamScaling` exists
(`Linear` | `Exponential`, `effects/mod.rs:145`) but `from_lisp_manifest` sets
`scaling: Linear` **uniformly for every custom dgenlisp instrument**
(`effects/mod.rs:7138-7158`) — which is every instrument the sound palette actually
shows. Taking `.scaling` at face value therefore means differencing every cutoff and
every envelope time linearly, and the low end of those ranges goes dark. This is the
biggest gap between the design and the codebase.

Resolution order:

1. If `descriptor.stored_to_user` / `user_to_stored` genuinely implement the display
   curve, use `stored_to_user` and normalize in user space — that is exactly the right
   hook and costs nothing. **Verify this before building anything else**; the pair is
   used by the UI sync layer but its semantics need confirming.
2. Otherwise infer from `ParamKind::Continuous { unit }` plus the range:
   ```
   taper(p) = if p.scaling == Exponential            { Exponential }
              else if unit ∈ {Hz, kHz, ms, s} && min > 0 && max/min >= 50 { Log }
              else                                    { Linear }
   ```
   The `max/min >= 50` test is what distinguishes a real frequency sweep from a narrow
   detune range that happens to be in Hz.
3. Failing both, `Linear`, and accept that low-end differences under-read.

```
t(p, v) = match taper(p) {
    Linear      => (v - min) / (max - min),
    Log         => (ln v - ln min) / (ln max - ln min),
    Exponential => sqrt((v - min) / (max - min)),      // matches ParamScaling::Exponential
}                                                      // → [0, 1]
```

A per-instrument taper override table in the manifest would beat all of this and is
cheap to author; worth considering as a follow-on, since the same information would
improve every knob in the app, not just the glyph.

`Boolean` and `Enum` parameters skip this entirely and go straight to §4.4.

### 4.2 Cohort deviation scale **[rev 2 — replaces MAD spread]**

For each slot `p`, over the cohort `C` and against the reference `R`:

```
devs    = [ |t(p, patch) - t(p, R)| for patch in C ]
scale_p = max( quantile(devs, 0.9),  SCALE_FLOOR = 0.05 )
```

**Rev 1 used a MAD of the cohort's *values*, and that was the central bug.** In the
common palette shape — several near-identical patches plus one or two variants — the
median absolute deviation of the values is ~0, every parameter pins to the floor, and
the variant divides by that floor and saturates. The result is bimodal: blank tiles and
one amoeba.

The corrected quantity measures the *deviations themselves*, so it answers the question
actually being asked: **how far does this patch differ, relative to the largest
differences present in this palette?** The most-deviating patch reads near 1.0 on each
parameter it moves, and every other patch scales proportionally beneath it. No collapse,
no bimodality, and it degrades gracefully — a cohort where everything differs equally
just normalizes to a common scale.

The 90th percentile rather than the max so that one pathological patch doesn't
compress everything else, while still tracking the top of the distribution rather than
its centre. With small cohorts, interpolate rather than index-select.

Note this makes the scale **reference-dependent**, so cohort statistics must be rebuilt
when the selection changes. They are cheap (§9.2); the alternative — a
reference-independent scale — reintroduces the collapse.

### 4.3 The absolute gate **[rev 2 — replaces the spread floor]**

Relative scale alone would still amplify an inaudible difference whenever it happens to
be the largest one present. A second, *absolute* test rejects that:

```
ABS_GATE = 0.05      // 5% of a knob's travel, in taper space

if |t(p, subject) - t(p, R)| < ABS_GATE:  d = 0
```

The two tests do different jobs and both are needed. `ABS_GATE` asserts *below 5% of a
knob's travel we do not care, no matter how much it dominates this palette*; `scale_p`
then decides how loud the differences that survive should be, relative to each other.
Rev 1 tried to make one constant serve both roles and it could not.

`ABS_GATE` is the first constant to adjust if glyphs feel noisy, and `SCALE_FLOOR` the
first if they feel flat.

### 4.4 Deviation

```
Continuous:  rel = |t(p, subject) - t(p, R)| / scale_p        // 1.0 ≈ the palette's largest
Discrete:    rel = (subject.value(p) == R.value(p)) ? 0 : 1.0
Boolean:     same as Discrete

sign = sgn(t(p, subject) - t(p, R))                           // 0 for discrete
```

Discrete parameters have no magnitude — a different waveform is not "1.4 waveforms
away" — so any change registers at full scale. Structural changes *should* shout;
they're usually the biggest audible difference in the patch.

### 4.5 Response curve

```
DEV_GAMMA = 0.65     // < 1 lifts small deviations

d = clamp(rel, 0, 1) ^ DEV_GAMMA     // → [0, 1], with d = 0 forced by §4.3
```

The dead zone now lives entirely in §4.3's absolute gate, which is where it belongs;
rev 1's `DEV_KNEE`/`DEV_FULL` in σ units are gone along with the σ scale itself.
`DEV_GAMMA < 1` expands the low end so a modest difference is clearly visible rather
than a faint speck — this is where "make small differences legible" is paid for, and it
is monotone, so requirement 2 survives.

### 4.6 Ink cap **[rev 2]**

```
MAX_LIT = 5      // [rev 3] each lit parameter is now its own shaded layer
```

Keep the `MAX_LIT` highest-`d` slots; force the rest to zero. **[rev 3]** Lowered from 7
because a lit parameter now costs a whole layer with its own normal estimate, so this
constant bounds shader cost as well as visual complexity — and pieces of 1–5 cells carry
far more ink per lit parameter than rev 2's single discs did. Nothing in rev 1 bounded
how much of the glyph could light up at once, so a patch differing in twenty parameters
lit twenty cells, which — combined with the §6.1 radius bug — is what produced the
amoeba.

The cap does more than bound complexity: it makes the glyph *say something specific*.
"Here are the seven things that most distinguish this patch" is a far more useful
statement than an undifferentiated wash, and it holds the visual complexity roughly
constant across the palette so tiles stay mutually comparable.

Ties at the cut are broken by slot order, so the choice is deterministic.

Output: a **deviation vector** `d ∈ [0,1]^n` with at most `MAX_LIT` non-zero entries,
plus a sign vector. That is everything the renderer consumes.

---

## 5. Layers

**[rev 3]** The layer model now mirrors the original's: a dark substrate, then **one
layer per lit parameter**, each layer a welded polyomino anchored into the *shared*
lattice so layers interpenetrate.

### 5.1 Layer 0 — the substrate **[rev 3 — replaces rev 2's address grid]**

The substrate occupies the **highest-valued `SUBSTRATE_FILL` fraction of this patch's
slots**, ranked by absolute taper value, and draws a disc at each:

```
SUBSTRATE_FILL = 0.55
R_SUB_MIN = 0.155      // gap 0.057 — welds
R_SUB_MAX = 0.185      // gap −0.003 — overlapping
r_sub(p)  = R_SUB_MIN + t(p, subject) * (R_SUB_MAX - R_SUB_MIN)
```

**Occupancy is the shape channel; radius is only surface texture.** This is the same
lesson as §6.1, one layer down, and the first rev 3 implementation got it wrong: it
occupied *every* assigned slot and put all the variation on radius. Because the entire
band welds by construction, that produces a featureless filled rectangle with a slightly
wobbly edge, identical across every patch in the palette — the variation is real in the
data and invisible on screen. Occupying a fraction of the slots instead gives the mass
an irregular polyomino silhouette with genuine negative space, which both varies
visibly between patches and leaves the accents somewhere to read against.

Ranking *within the patch* rather than thresholding an absolute value keeps the fill
fraction constant across instruments whose defaults happen to sit high or low — a fixed
`t > 0.5` test would produce a nearly-empty substrate on one instrument and a nearly-full
one on another.

Rendered dark and low-contrast, lit with the same bevel as the accents so it reads as
material rather than backdrop. The mass may be disconnected; islands are fine and read
well.

This is the third attempt at this layer and the reasoning is worth keeping:

- **Rev 1** derived it from absolute values at `R_BASE = 0.15` but drew it as
  *sub-bridging beads* — a scatter of large dots, near-identical across tiles, using the
  same shape vocabulary as the accents. It read as noise competing with the signal.
- **Rev 2** over-corrected to a uniform faint address grid. That fixed the noise by
  removing all information, and a cohort of similar patches became a field of identical
  gray dots with no character at all.
- **Rev 3** keeps rev 1's *source* (absolute values, continuous) and rev 2's *restraint*
  (dark, subordinate, never competing) while fixing what was actually wrong in both: the
  form. A welded mass is subordinate and characterful at the same time; a scatter of
  dots is neither.

Three consequences, all wanted. Two patches with identical values produce an identical
substrate — correct, and now a shape rather than a void. Two patches that differ produce
visibly different silhouettes *before any accent lights up*. And because radius is
continuous, near-identical patches produce near-identical masses, so the metric survives
without hashing or quantization pop.

Unassigned lattice positions draw nothing. The lattice's extent is legible from the
substrate itself.

### 5.2 Layers 1..n — one per lit parameter

Each lit parameter (§4.6, at most `MAX_LIT`) becomes **its own layer**: a contiguous
polyomino (§6.1) anchored at that parameter's slot, its primitives smooth-unioned at
fixed radius, shaded with its own normal estimate, then composited back-to-front with a
crease against everything already drawn.

Pieces are anchored into the **shared** lattice and may overlap each other and the
substrate. That interpenetration is not a defect to be designed around — it is the
original's second tier of visual richness, and the crease pass is what makes it read as
stacked material rather than flat overlapping circles. Rev 2's per-slot layer encoding
made it structurally impossible.

Layer order is slot order: deterministic, and stable across the cohort.

### 5.3 Hue

Hue is per *piece*, taken from the parameter's group, so it is a legend rather than
decoration. The palette lives as a constant in both the Rust module and the shader
(dual-maintained; the shader needs it to index a hue without spending a uniform slot per
layer).

Assignment is stable per group within an instrument — named groups take fixed hues, and
free-form groups take palette slots by sorted position, so adding a parameter to an
existing group never re-colors a glyph. Params with no derivable group take a neutral
tone that must stay clearly *un*-hued, since it means "unclassified".

**[rev 3]** This supersedes rev 2's "two accent groups, everything else neutral", which
was forced by having only two tint uniforms and produced mostly-neutral glyphs whenever
a patch differed broadly (the §12 open question). With the palette in the shader and a
3-bit hue index per piece, every piece can carry its own group's hue.

**[rev 2, from implementation]** The spores lighting model adds its diffuse and specular
terms *achromatically* — an operator-precedence artifact in the original
([algorithm doc §6.4](./sdf-blob-glyph-algorithm.md)). Ported verbatim it desaturates
every cell toward white, which is tolerable when hue is decoration and fatal when hue is
the group legend. Damp that term to ~0.35 of its original weight and raise the tinted
diffuse to ~0.85; the bevel survives, the hue reads.

**[rev 3, from implementation]** No instrument in the repo sets `ui_metadata.group`, so
the *fallback* is the real path and it must be better than a substring search: matching
anywhere in the name classifies `opa_level_db` as Mix because it contains "level", when
it is plainly operator A. Key off the name's **leading snake_case token** instead
(`opa`, `filter`, `lfo`, `fenv`, `shaper`), mapping known audio words onto the named
groups and everything else to `Other(token)`.

---

## 6. Geometry channels

**[rev 3]** How `d` and `sign` become shapes. The governing change from rev 2: **radius
is fixed and magnitude rides occupancy.**

### 6.0 The fusion arithmetic, stated correctly

Two equal discs of radius `R` at centre distance `D` have a surface gap `g = D − 2R`,
and each reads `g/2` at their midpoint. The smooth-min's maximum sag is
`0.5·k/1.55 = 0.32258·k`. They therefore merge iff

```
g ≤ 0.6452 · k
```

Rev 2 used `g ≤ k`, overestimating the fusion budget by 1.55×.

The lattice has three adjacencies, not one — and the vertical/diagonal pair is the
*dominant* one in the piece vocabulary (11 of the original's 14 types use it):

| adjacency | offset | D |
| --- | --- | --- |
| horizontal | Δcol 1 | 0.3672 |
| vertical | Δrow 1 (with ±0.18 stagger) | 0.40572 |
| near diagonal | Δcol 1, Δrow 1 | 0.40896 |

At `R = 0.18` and `k = 0.155` (threshold gap **0.100**) the gaps are 0.0072, 0.0457 and
0.0490 — 7%, 46% and 49% of the budget. Everything welds, solidly: the neck between two
cells sits 0.025–0.046 *inside* the surface. Fusion in the original is universal, not an
event.

Any future retune must check **all three** distances. Rev 2 checked only the first and
shipped a configuration in which vertical welding was impossible at any deviation.

### 6.1 Occupancy — the magnitude channel

```
R_CELL = 0.18       // fixed for every accent primitive, never varies
k      = 0.155      // fixed
```

Deviation selects a **piece**: a contiguous polyomino of 1–5 cells, anchored at the
parameter's slot, whose primitives are smooth-unioned into one welded blob.

```
tier    = clamp(floor(d * 5), 0, 4)      // 1, 2, 3, 4, 5 cells
variant = slot % 3                       // stable per parameter, no hashing
```

Tier gives magnitude; variant gives each parameter a characteristic growth shape so the
glyph has variety without a hash destroying the metric. The vocabulary, three variants
per tier:

| tier | cells | variants |
| --- | --- | --- |
| 0 | 1 | disc |
| 1 | 2 | capsule · vertical pair · diagonal pair |
| 2 | 3 | capsule+disc · L · vertical run |
| 3 | 4 | two stacked capsules · 2×2 square · capsule+2 discs |
| 4 | 5 | 2 capsules+disc · P-pentomino · capsule+3 discs |

**Capsules are a normal outcome, not a marker.** They appear in 6 of the 15 entries —
40%, comfortably above the original's ~26% rate. A capsule is a stadium spanning two horizontally
adjacent cell centres at radius `R_CELL` — the two-cell weld expressed as one *convex*
primitive with no waist, which is where the elongated lobes come from. This supersedes
rev 2's linked-pair capsule, which required a `link` plus same group plus adjacency and
therefore almost never fired.

The quantization is deliberate and is the point: a parameter crossing a tier boundary
**grows a lobe** rather than swelling smoothly. Topology change is the strong
pre-attentive signal; §6.1a keeps the reading continuous between tiers.

Primitives falling outside the lattice are simply not drawn, so edge parameters render
partial pieces. `mirror = anchor_col ≥ COLS/2` flips a piece's horizontal growth
direction so it grows inward, which keeps most of it on-tile.

### 6.1a Luminance — the continuous channel

```
tint = group_hue * (0.5 + 0.5 * magnitude)      // magnitude quantized to 3 bits
```

Piece size is coarse (five steps); luminance fills in between them, so the mapping still
reads as continuous. Brightness is fast to read, precise at small sizes, and — unlike
radius — cannot change topology, which is exactly why it, and not radius, is the right
partner for an occupancy-based magnitude channel.

Implementation note for the SDF renderer: shade per *piece*, not per pixel-of-a-layer —
each piece is its own layer with its own normal estimate, as in the original.

### 6.2 Sign — hue temperature, not position **[rev 3]**

Rev 1–2 nudged a cell ±`0.055·d` within its slot. On two vertically adjacent
opposite-sign cells that adds 0.11 to a 0.3636 step, giving a gap of 0.187 — **2.9× the
entire fusion budget**. The sign channel alone de-fused more than the fillet could
close.

Sign now rides a subtle warm/cool shift of the piece's hue: positive deviations warm
(`×(1.08, 1.00, 0.92)`), negative cool (`×(0.92, 1.00, 1.10)`). It costs no geometry,
survives fusion, and reads at cluster scale as a palette-wide temperature. Discrete
parameters, which have no sign, stay neutral.

---

## 7. Modes

| Mode | Reference | When |
| --- | --- | --- |
| **Anchor** (default) **[rev 2]** | the **first** patch in cohort order | all normal browsing |
| **Centroid** | cohort median (per-slot median in taper space) | overview; also stable |
| **Selection-relative** | currently-selected patch | explicit A/B against one patch |
| **Absolute** | instrument default patch | "how far from stock is each of these" |

All four are the same code path with a different reference vector; centroid and absolute
references are synthetic patches, not real ones.

**[rev 2] The default is the anchor, not the selection.** Rev 1 defaulted to
selection-relative, which meant every glyph in the palette changed shape whenever you
clicked a different tile. That is disorienting in exactly the situation the glyph is
for — scanning a set to find one — because the reader can never build a stable mental
image of any tile. Diffing against a fixed anchor keeps each patch's shape a property of
*the patch*, so you learn the palette instead of re-reading it. Selection then only
changes the tile border, which is what selection should do.

Three consequences, all good:

- **Cohort statistics become selection-independent** (§4.2's scale depends on the
  reference), so nothing invalidates when you click. This removes the main caching
  liability in §9.2 outright.
- **The anchor's own tile is the null glyph** — bare address grid. Mark it with a thin
  centered ring so it reads as "this is the zero point," not "this patch is empty."
- Cohort order must be **stable**, since the anchor is defined by it. Use the palette's
  existing entry order, which is pool order, not a sort that varies with selection or
  playback state.

Selection-relative remains available for a deliberate A/B, and is where the §9.3 morph
transition earns its keep. Centroid is the better default for any *non*-palette surface
(browser lists, arrangement clips) where there is no meaningful first element.

---

## 8. Degradation

What is actually available, per §2.1, and what to do when it isn't:

| Field | Availability | Fallback / cost |
| --- | --- | --- |
| `range` | **always** (`.min`/`.max`) | — |
| `kind` | **always** | — |
| `order` | **always** (positional) | — |
| `group` | **usually** — `ui_metadata` is `Option`, and `group` inside it is `Option` | params without a group land in a synthetic `"other"`; an instrument with *no* groups at all loses localization (req. 3) but still differentiates |
| `taper` | **effectively never** for custom instruments (§4.1) | inference chain in §4.1; worst case `Linear` and low-end differences under-read |
| `link` | **sometimes** via `env`/`role` | no capsules; lose one channel |
| `weight` | never | uniform weighting (§12) |

Cohort of size 1, or all patches identical: every spread hits the floor, every deviation
is 0, all glyphs are base-layer only. Correct behaviour — there is nothing to
differentiate, and the glyph should say so rather than manufacture contrast.

---

## 9. Rendering and integration

The existing sound-glyph pipeline is a good fit structurally and a **bad fit for the
renderer**. Structure first, then the problem.

### 9.0 Existing pipeline

Geometry is computed in `crates/sequencer` and rendered in `crates/eseqlisp`; they
communicate only through a global keyed frame store, so the widget never computes
anything.

| Stage | Location |
| --- | --- |
| geometry lib | `crates/sequencer/src/sound_glyph/` (`extract.rs`, `geometry.rs`, `stock.rs`) |
| host / cache / publish | `crates/sequencer/src/ui/state_values/sound_palette.rs` (`GlyphFrames`, `:43-171`) |
| transport | `crates/eseqlisp/src/sound_glyph_data.rs` — `SoundGlyphFrame { revision, strokes, marks }`, global `OnceLock<Mutex<HashMap<String, Arc<Frame>>>>` |
| widget | `crates/eseqlisp/src/widget_render/sound_glyph.rs` — `"sound-glyph"`, `build_metal_primitives` `:80` |
| draw site | `crates/sequencer/ui/sound-palette.lisp:148-152` |
| cohort source | `App::sound_palette_entries` — `crates/sequencer/src/app/sound_palette.rs:302` |

Everything the delta model needs from the app is already there:

- **Cohort** = the `Vec<PaletteEntry>` the palette already builds (`sound_palette.rs:302`).
- **Reference** = the entry with `is_current == true` (`:381`) — already computed, since
  it drives the tile highlight. Centroid and absolute modes (§7) need no new plumbing at
  all.
- **Frame key** = existing `glyph_key(track, patch)` = `sound-glyph:track:{t}:patch:{p}`
  (`:56-58`), passed to the widget as `:source` via the entry's `glyph-key` field
  (`:258`).

So the delta glyph replaces `sound_glyph::{extract_skeleton, resolve_geometry}` and the
`SoundGlyphFrame` payload, and leaves the transport, the widget registration, the lisp
call site, and the retain/prune logic (`retain_sound_glyph_frames`, `:170`) intact.

### 9.1 The renderer — resolved as a Metal SDF shader **[rev 3]**

The palette's primitive vocabulary (`MetalPrimitive::Triangle`, `::Circle`) cannot
evaluate a smooth-union field per pixel, so rev 1 weighed three options: circles-only,
CPU contour-band tessellation, or a real SDF fragment shader. **The shader was built**
(`widget_render/sound_glyph.rs`, `DELTA_GLYPH_SHADER`), and rev 3 vindicates that
choice — banded contours would have discretized exactly the molten welds and creases
that carry the look.

The frame payload is a compact lattice description, not geometry; all shape generation
lives in the shader. Uniform packing (18 words, exact 24-bit integers only — depending
on NaN payload bits is unsafe across drivers):

| words | contents |
| --- | --- |
| 0–4 | substrate: five 4-bit slot levels per word, 25 slots |
| 5–9 | up to `MAX_LIT` piece records, 18 bits each |
| `color_a` | substrate tint |
| `color_d` | `cols`, `rows`, unused, anchor flag |
| `corner_radius` | incompatible flag |

Piece record layout: `slot(5) | piece(4) | hue(3) | magnitude(3) | mirror(1) |
negative(1) | present(1)`. The hue palette is a shader constant (`DG_HUES`,
dual-maintained with `delta_glyph::GROUP_PALETTE`) rather than a uniform, which is what
lets every piece carry its own group's hue instead of rev 2's two tint slots.

Cost: one substrate field plus up to five piece fields, each with a 4-tap central
difference for its normal. That is ~5× rev 2's shader work and is the reason `MAX_LIT`
dropped to 5. `registered_widget_shaders_compile_in_metal` covers compilation.

### 9.2 Caching

`GlyphFrames` (`sound_palette.rs:43-54`) already fingerprints each glyph as a hash over
`identity + every bit of patch.instrument_slot.defaults` (`:112-117`) and skips
republishing when it's unchanged (`:119`). That skip exists for a real reason:
`publish_sound_glyph_frame` calls `bump_widget_state_generation()`
(`sound_glyph_data.rs:46`), invalidating the compiled primitive cache and repainting
everything — so a naive republish every sync frame would repaint the whole UI
continuously.

The delta model widens that fingerprint to:

```
(identity, subject.defaults, cohort_signature, mode)
```

where `cohort_signature` hashes the ordered `(patch_id, defaults_hash)` list of the
compatible cohort — which already includes the anchor, since the anchor is just
`cohort[0]` (§7). This is a **structural change from the current glyph**, which is a
pure function of one patch and caches per patch forever. A delta glyph invalidates
*every tile* whenever any patch in the cohort is edited or the cohort membership
changes.

**[rev 2]** It does *not* invalidate on selection change, because anchor mode's
reference is selection-independent. That was rev 1's main caching liability and it is
now gone: clicking a tile repaints a border, nothing more. The mitigations below still
matter for edits and for palette open, but the steady-state browsing cost is zero.

Mitigations, in order of importance:

- **Batch the generation bump.** Editing a patch, or opening the palette, dirties every
  frame at once. Publish all of them, then bump the widget generation **once**, rather
  than once per frame — the current `publish_sound_glyph_frame` bumps per call, which
  would mean N bumps for an N-tile palette. This needs a
  `publish_sound_glyph_frames(batch)` variant or a deferred-bump guard.
- **Split the cache in two.** Deviation vectors (§4) invalidate constantly but are cheap
  — O(cohort × slots) of arithmetic, no tessellation. Tessellated geometry is expensive
  but keys on the *quantized* deviation vector, so an edit that doesn't move any `d` past
  its quantum doesn't re-tessellate.
- **Compute cohort statistics once per sync**, not per tile — spreads are shared across
  the whole palette. Per-tile work then reduces to a vector subtract, a divide, and the
  response curve.
- **Recompute only while the palette is open.** `sync_glyph_frames` is already called
  from `sync_sound_palette` (`:311`) only in that case, and closing clears everything
  (`:332-333`). Keep that.

Also inherited and worth fixing while in here: the skeleton cache keys on instrument
identity and goes stale when an instrument is re-saved until app restart (flagged at
`sound_palette.rs:45-47`, accepted for P2). The delta glyph's schema-derived slot layout
has the same exposure — a re-saved instrument can change the param list under a cached
layout. Key the slot layout on the descriptor's param-name list hash, not just identity.

### 9.3 Transitions

Because the field is continuous in `d`, a selection change can **morph** rather than cut:
interpolate the deviation vectors over ~150 ms and re-render. Cells grow in and out,
lobes fuse and split.

**[rev 2]** With anchor mode this is no longer needed for selection — nothing moves when
you click. It still applies where the reference genuinely changes: editing a patch,
switching cohorts, or an explicit switch into selection-relative mode, where watching
the palette reorganize around a new reference is the whole affordance.

---

## 10. Tuning constants, collected

| Constant | Value | Governs |
| --- | --- | --- |
| `ABS_GATE` | 0.05 | noise rejection — **tune this first** (§4.3) |
| `SCALE_FLOOR` | 0.05 | floor on the deviation scale; raise if glyphs feel flat (§4.2) |
| `SCALE_QUANTILE` | 0.9 | which part of the deviation distribution sets full scale (§4.2) |
| `DEV_GAMMA` | 0.65 | small-difference lift |
| `MAX_LIT` | 5 | ink cap — accent pieces, hence layers, per glyph (§4.6) |
| `R_CELL` | **0.18, fixed** | accent primitive radius. Never varies — see §6.1 |
| `k` | **0.155, fixed** | fillet. Threshold gap `0.6452·k = 0.100` (§6.0) |
| `R_SUB_MIN` / `R_SUB_MAX` | 0.155 / 0.185 | substrate band, entirely inside the fusion zone (§5.1) |
| `SUBSTRATE_FILL` | 0.55 | fraction of slots the substrate occupies — its shape channel (§5.1) |
| piece tiers × variants | 5 × 3 | shape vocabulary; tier from `d`, variant from `slot % 3` (§6.1) |
| capsule share | 6 of 15 | vs the original's ~26% |
| sign shift | `×(1.08,1.00,0.92)` / `×(0.92,1.00,1.10)` | warm/cool, not positional (§6.2) |
| luminance floor | 0.5 | tint at magnitude 0 (§6.1a) |
| `blur` | 0.020 | edge softness (from algorithm doc) |
| crease erosion | 0.05 | seam depth (from algorithm doc) |
| max slots | 25 (5×5) | legibility floor at the palette's ~90–110 px tile |
| reference | first patch in cohort order | anchor mode (§7) |

Fusion check for the three adjacencies at `R_CELL = 0.18`, `k = 0.155` (§6.0):

| adjacency | D | gap | vs threshold 0.100 |
| --- | --- | --- | --- |
| horizontal | 0.3672 | 0.0072 | 7% |
| vertical | 0.40572 | 0.0457 | 46% |
| diagonal | 0.40896 | 0.0490 | 49% |

All three weld solidly. These are asserted as `const` invariants in `delta_glyph.rs`, so
a future retune that breaks any of them fails to compile rather than silently shipping
rev 2's isolated-circles behaviour.

All lengths are in unscaled uv units and are multiplied by `fit` (§3.5) at render time.
The `fit` extent must include the ±0.09 row stagger, or staggered cells clip at the tile edge.

---

## 11. Build order

**[rev 3]** Built and rendering. What shipped, in order:

1. Normalization (§4) — quantile scale, absolute gate, response curve, ink cap.
2. Cohort-level layout, hues, and lattice (§3.4), with a test asserting two subjects in
   one cohort produce identical layouts.
3. Anchor reference (§7).
4. Metal SDF shader (§9.1).
5. Piece vocabulary and the substrate (§5, §6) — the rev 3 geometry rewrite.

Regression tests worth keeping, since every failure so far was invisible to the previous
revision's suite:

- a mostly-identical cohort produces *graded* output, not one saturated tile
- two subjects in one cohort produce identical layouts and identical hues per group
- deviation grows the piece **tier**, monotonically
- **every piece in the vocabulary is contiguous** — a disconnected piece cannot weld,
  which was rev 2's core defect
- the substrate tracks the absolute parameter vector: same values → same form, different
  values → different form
- the §6.0 fusion invariants are `const` assertions, so breaking them fails the build

---

## 12. Open questions

- **Base layer content.** Absolute-value-driven (§5.1) or instrument-topology-driven?
  The latter is prettier and more "species-like" but requires the sound-glyph extraction
  library to stay in the picture. Decide after step 2 shows how much visual room the
  base actually has.
- **Group palette.** The six hues in §5.2 are placeholders and need a real pass against
  the app's existing color language, including the scene accent colors already used on
  palette tile headers (which will sit directly above the glyph and must not fight it).
- **Is two delta layers enough?** Three groups deviating is common; folding the third
  into the second loses localization for exactly the busiest patches. Alternative:
  three layers with the third at reduced saturation. Needs eyes on it.
- **Perceptual weighting per parameter.** Some parameters are audibly dominant (cutoff,
  decay) and some are not (a mod-matrix trim). A static per-parameter weight in the
  schema would sharpen the glyph considerably, but it's per-instrument authoring work.
  Defer, but leave a `weight: f32 = 1.0` field in the schema so it can be added without
  a migration.
- **Does the snap threshold survive at 60 px?** The palette may want a smaller tile in
  some contexts. Below ~20 px per cell the fillets stop resolving and the topology cue
  dies; the fallback is probably a reduced lattice for small sizes, which breaks slot
  stability across sizes. Note §3.2 already sits at the floor (~20 px at 5×5) on the
  current tile, so there is no headroom — a smaller surface needs a genuinely different
  layout, not a scaled one.
- **P-locks are invisible.** The glyph reads `instrument_slot.defaults` only, matching
  the current implementation. Two patches whose entire difference lives in `plocks` or
  `key_locks` will render identically. That may be correct (a p-lock is arguably pattern
  data, not sound identity) or may be a real blind spot for how the palette is used.
  Decide deliberately rather than by inheritance.
- **Tensor params have no delta metric.** Wavetables and MSEG shapes are excluded (§3.3).
  A shape distance (L2 over the normalized tensor, or a few spectral moments) would fold
  them in as ordinary slots and is not hard — but it's a separate design question.
- **Aggregation is the common case, not the exception.** With 40–80 visible slots against
  a 25-slot lattice, most glyphs are mostly aggregates. Verify at step 3 of §11 that the
  aggregated view still discriminates; if it doesn't, the answer is probably
  per-parameter weighting (above) to pick the 25 that matter rather than RMS-ing
  everything.
- ~~**Is a two-hue color budget right?**~~ **[rev 3] Resolved** by moving the palette
  into the shader and giving every piece its own group hue (§5.3).
- **Shader cost at palette scale.** Six fields per glyph, each with a 4-tap normal, times
  ~20 visible tiles. It renders fine in the capture harness, but has not been profiled in
  a live 60fps palette. If it bites, the cheapest win is caching each glyph to a texture
  since the content only changes on edit.
- **Piece variant is `slot % 3`.** Deterministic and stable, but arbitrary — it means two
  adjacent parameters reliably grow in different directions, which is good for variety
  and meaningless as information. A variant chosen to avoid collisions with neighbouring
  pieces would look more deliberate.
- **Taper override table.** §4.1's inference chain is a workaround. A `taper:` field in
  the dgenlisp param manifest would fix it properly and improve every knob in the app.
  Worth costing separately.
