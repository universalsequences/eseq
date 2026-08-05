# SDF Blob Glyph Algorithm (reverse-engineered from `spores-next`)

Complete, implementation-level formalization of the generative glyph algorithm in
`~/code/spores-next`. This document describes **only the algorithm** — how it turns a
seed into pixels — with every constant, expression, and quirk preserved exactly as the
original produces them. It deliberately says nothing about how eseq patches would map
onto it; that mapping is a later exercise.

Source files (all under `~/code/spores-next/src/`):

| File | Role |
| --- | --- |
| `pages/graphic.tsx` | page entry |
| `components/card/GraphicComponent.tsx` | host, canvas cache, IPFS upload |
| `components/card/GenerativeGraphic.tsx` | seed → numbers → (algo, colors) |
| `hooks/shaders/usePuzzle.ts` | top-level composition: grid params, smooth-union factor |
| `hooks/shaders/grid.ts` | 4×4 shape lattice |
| `hooks/shaders/puzzle.ts` | piece (polyomino) table, partitioning, per-layer lighting |
| `hooks/shaders/sdf.ts` | rounded-box SDF |
| `hooks/shaders/scene.ts` | union / **smooth union** fold |
| `hooks/shaders/2d-lighting.ts` | normal estimation from a 2D height field |
| `hooks/shaders/layers.ts` | layer compositing + intersection ("crease") pass |
| `hooks/shaders/shader.ts` | GLSL boilerplate (perlin, hsl, uv) |
| `hooks/shaders/gen.ts`, `math.ts`, `functions.ts`, `float.ts` | expression-tree codegen |

The whole thing is a **codegen system**: TypeScript builds a GLSL expression tree
(`Fragment` = `{code, uniforms, variables, functions}`), emits one fragment shader
string, and renders it on a full-screen quad via three.js (`useShader.ts`,
`resolution: 1`, size 400×400 in the demo). Nothing is animated; there is no `time`
dependence in the glyph path.

---

## 0. Pipeline overview

```
seed string
  └─► filenameToNumbers  ─► 9 ints in [0,18] ∪ {-1}   (called TWICE, independently)
        ├─ call A ─► numbersToRGBColors ─► 3 RGB colors
        └─ call B ─► algo: PieceType[9]
                       │
                       ▼
        createPuzzle(algo, 4, 4, shapes)  ─► Puzzle = Shape[][]   (one Shape[] per algo entry)
                       │                       shapes = gridRect(4×4 lattice of circles)
                       ▼
        puzzleLayers(puzzle, {SMOOTH_UNION, k=0.155}, colors) ─► Layer[]
                       │   per layer: sceneN() = smooth-union fold of that partition's SDFs
                       │              height field → central-difference normal
                       │              2 diffuse lights + 2 Blinn-Phong speculars → color Fragment
                       ▼
        generateLayers(layers, bg=vec4(0,0,0,0.0491))
                       │   painter's algorithm, smoothstep(0, 0.020, sdf) coverage
                       │   + intersection pass vs union-so-far (dilated 0.05) for creases
                       ▼
        boilerplate(fragColor) ─► GLSL ─► gl_FragColor
```

Key structural idea: **the glyph is a set of polyomino "pieces" laid on a 4×4 lattice of
circles.** Each piece is smooth-unioned into a single blob; each blob is one lit,
colored layer; layers are stacked back-to-front with crease shading where a layer
overlaps everything already drawn.

---

## 1. Coordinate space

`gen.ts::uvFragment` emits, once, at the top of `main()` and at the top of every
generated function that references uv:

```glsl
vec2 uv = (1.0*gl_FragCoord.xy - vec2(width, height)) / height;
```

`width`/`height` are uniforms set to the canvas pixel dimensions (`useShader.ts`).

For a square canvas this maps the viewport to **`uv ∈ [-1, 0] × [-1, 0]`** — the
subtraction uses the *full* width/height rather than half, so the origin sits at the
**top-right** corner of the visible area, not the center. This is almost certainly
unintentional (the conventional form is `(2.0*gl_FragCoord.xy - resolution)/height`),
but the constants in §2 were tuned against it, so it is part of the algorithm as it
stands. Consequence: lattice cells with positive x or y are cropped (see §2.3).

Notation below: `p` is a point in uv space; distances are in uv units, where 1 unit =
half the canvas height.

---

## 2. The lattice (`grid.ts`, `usePuzzle.ts`)

### 2.1 Parameters (exact, from `usePuzzle.ts`)

```ts
gridRect(
  { xCount: 4, yCount: 4,
    spacing:   { x: 1.04, y: 1.02 },
    alternate: { x: 1,    y: 0    } },
  rect({ x: 0, y: 0, width: 0.18, height: 0.18, radius: 0.18 })
)
```

`spacing` is a *multiplier on the base size*, not an absolute gap. `alternate` is a
staggering offset, also in units of the base size.

### 2.2 Generated positions

```
stepX = width  + spacing.x * width  = 0.18 * 2.04 = 0.3672
stepY = height + spacing.y * height = 0.18 * 2.02 = 0.3636

x1 = x0 - (xCount/2)*stepX + width  - 0.5*alternate.x*width
   = 0   - 2*0.3672       + 0.18   - 0.09              = -0.6444
y1 = y0 - (yCount/2)*stepY + height - 0.5*alternate.y*height
   = 0   - 2*0.3636       + 0.18   - 0                 = -0.5472

for i in 0..3 (columns, x):
  for j in 0..3 (rows, y):
    offsetX = (j % 2 == 1) ? alternate.x : 0      // odd rows shift right by 0.18
    offsetY = (i % 2 == 1) ? alternate.y : 0      // alternate.y = 0 ⇒ no-op
    shapes[i*4 + j] = { x: x1 + i*stepX + offsetX*0.18,
                        y: y1 + j*stepY + offsetY*0.18,
                        width: 0.18, height: 0.18, radius: 0.18 }
```

**Index convention (matters for the piece table): `index = column*4 + row`. `+1` moves
one row up (+y), `+4` moves one column right (+x).**

Concrete values:

| | x (even row j) | x (odd row j, +0.18) |
| --- | --- | --- |
| i=0 | −0.6444 | −0.4644 |
| i=1 | −0.2772 | −0.0972 |
| i=2 | +0.0900 | +0.2700 |
| i=3 | +0.4572 | +0.6372 |

| | y |
| --- | --- |
| j=0 | −0.5472 |
| j=1 | −0.1836 |
| j=2 | +0.1800 |
| j=3 | +0.5436 |

### 2.3 What is actually on screen

With `uv ∈ [-1,0]²` and cell radius 0.18, columns i=0,1 are fully visible, i=2 is
clipped at the right edge, i=3 is entirely off-screen; rows j=0,1 are visible, j=2 is
clipped at the top, j=3 is off-screen. In practice only low `shapes[]` indices are ever
used (see §4), so the visible cluster is ~3 wide × 2–3 tall, matching the reference
renders.

### 2.4 The primitive is a circle, not a rounded box

`sdf.ts::generateShapeFunction` emits Inigo Quilez's per-corner rounded box:

```glsl
float rectSDFn(vec2 p) {
    p -= vec2(cx, cy);
    vec4 r = vec4(0.18, 0.18, 0.18, 0.18);   // radius on all four corners
    vec2 b = vec2(0.18, 0.18);               // HALF-extents
    r.xy = (p.x > 0.0) ? r.xy : r.zw;
    r.x  = (p.y > 0.0) ? r.x  : r.y;
    vec2 q = abs(p) - b + r.x;
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r.x;
}
```

Because `radius == half-extent == 0.18`, the box degenerates to a **disc of radius
0.18**. `b` is a half-extent, so the base cell is 0.36 across and cells are spaced
0.3672 apart — i.e. neighbouring discs nearly touch (gap 0.0072), which is what makes
the smooth union in §5 fuse them into one continuous blob.

The only case where the primitive is *not* a circle is the "line" case (§4.2), where
`width` is doubled to 0.36 while `radius` stays 0.18 → a **horizontal capsule/stadium**
0.72 × 0.36.

---

## 3. Seed → numbers → colors (`GenerativeGraphic.tsx`)

### 3.1 `filenameToNumbers(name) → PieceType[9]`

```ts
name = name.replace(".mp3", "");
name = name.slice(0, 2) + name.split("").reverse().join("");   // prefix + full reverse
bytes = new TextEncoder().encode(name);                        // UTF-8

minuses = 0
for i in 0..bytes.length-1:
    num = bytes[i] % 19                       // 0..18
    if (minuses < 4 && Math.random() < 0.1):  // NON-DETERMINISTIC
        minuses++; push(-1)
    else:
        push(num)
    if (out.length == 9) break
while (out.length < 9) push(0)                // pad short names
```

Notes:
- The `Math.random()` branch makes the algorithm **non-deterministic**: up to 4 of the
  9 slots are randomly replaced by `-1`, which is *not* a valid `PieceType` and yields
  an empty partition (§4). This is the single largest source of visual variation
  between two renders of the same seed.
- The string mangling (`slice(0,2) + reverse`) exists to decorrelate the leading bytes,
  since only the first ≤9 bytes ever survive.
- Modulo 19 is used even though `getPiece` only defines 0..14; values 15–18 behave as
  `-1` (empty).

### 3.2 `numbersToRGBColors(nums[9]) → 3 RGB triples`

```ts
for i in 0..2:
    r = nums[3i]/18;  g = nums[3i+1]/18;  b = nums[3i+2]/18
    colors[i] = [ min(1, r*1.6), min(1, g*1.5), min(1, b*1.5) ]

// third color is pulled toward the second
colors[2] = [ 0.6*colors[1][0] + 0.4*colors[2][0],
              0.6*colors[1][1] + 0.4*colors[2][1],
              0.8*colors[1][2] + 0.2*colors[2][2] ]
```

The ×1.6/×1.5/×1.5 gain plus clamp is what pushes channels to saturation (the
characteristic acid-green / deep-violet pairs). The `colors[2]` blend keeps the third
layer family harmonically related to the second rather than fully independent.

Two alternate palette functions exist in the file (a commented-out
"dominant-channel = 1.0" variant and `numbersToRGBColors2` with an 80/20
saturated-vs-glacier-blue branch); **neither is wired up**. The live path is the one
above.

Colors become `vec4(r, g, b, 1)` fragments. The pre-seed default is:

```ts
[ vec4(0.1, 0.99, 0.1, 1), vec4(0.98, 0.22, 0.33, 1), vec4(0.97, 0.12, 0.13, 1) ]
```

### 3.3 Two independent draws

`GenerativeGraphic` calls `filenameToNumbers(props.name)` **twice** — once for colors,
once for `algo` — so the color numbers and the piece numbers get *different* random
`-1` injections and are not the same array.

---

## 4. Seed → pieces → partitions (`puzzle.ts::createPuzzle`)

```ts
createPuzzle(algo: PieceType[], width = 4, height = 4, shapes: Shape[]) -> Shape[][]
```

For each `algo[idx]` (idx = 0..8) exactly one partition is produced (possibly empty).
Two mutually exclusive branches:

### 4.1 Polyomino branch (default)

```ts
pieceIndices = getPiece(algo[idx], 4, 4)
partition = pieceIndices.map(pi => shapes[(idx + pi) % 16])
```

`idx` acts as the **placement offset** into the lattice — piece *n* is anchored at
lattice index *n*. `getPiece` returns index offsets in the `col*4 + row` scheme (`W` =
4 below):

| type | offsets | shape |
| --- | --- | --- |
| 0 | `[]` | empty |
| 1 | `[0]` | single cell |
| 2 | `[0, W]` | horizontal domino (see 4.2 — overridden) |
| 3 | `[0, 1]` | vertical domino (see 4.2 — overridden) |
| 4 | `[0, 1+W]` | diagonal pair |
| 5 | `[0, W, 2W]` | horizontal tromino |
| 6 | `[0, 1, 2]` | vertical tromino (overridden) |
| 7 | `[0, 1, 1+W]` | L-tromino (overridden) |
| 8 | `[0, 1, W]` | L-tromino (mirror) |
| 9 | `[0, 1+W, W]` | L/T-tromino (overridden) |
| 10 | `[0, 1, W, 1+W]` | 2×2 square |
| 11 | `[0, 1, 2, 3]` | vertical I-tetromino |
| 12 | `[0, 1, 2, W, 1+W]` | P-pentomino |
| 13 | `[0, 1, 2, 1+W, 2+W]` | P-pentomino (mirror) |
| 14 | `[0, 1, W, 1+W, 2+W]` | P-pentomino (offset) |
| else (incl. −1, 15–18) | `[]` | empty |

### 4.2 "Line" branch (overrides 4.1 for types 2, 3, 6, 7, 9)

`isLine(pieceType, idx)` returns a list of `{idx, width}` records; if non-empty the
polyomino branch is skipped entirely:

```ts
type 2 → [{idx,   w:2}]
type 3 → [{idx,   w:2}]
type 6 → [{idx,   w:1}, {idx+1, w:2}]
type 7 → [{idx,   w:2}, {idx+1, w:1}]
type 9 → [{idx,   w:2}, {idx+1, w:2}]
```

For each line record:

```ts
if (lineIdx >= lineWidth * height) break;     // see quirk below
base   = shapes[lineIdx % 16]
_width = base.width * lineWidth               // 0.18 or 0.36
push({ ...base,
       x: base.x + (lineWidth == 1 ? 0 : _width/2),   // +0.18 when doubled
       width: _width })
```

So a `w:2` line is a **stadium** of half-extents (0.36, 0.18) with radius 0.18, shifted
right by 0.18 so it grows rightward from its anchor cell — it spans the anchor cell and
its right neighbour. A `w:1` line is just the plain disc at that cell.

**Quirk (preserve or fix deliberately):** in the loop `for (let {idx, width} of lines)`
both `idx` and `width` **shadow** the enclosing `createPuzzle` parameters. The guard
`idx >= width * height` therefore means `lineIdx >= lineWidth * 4`, i.e. lines are
silently dropped when `lineIdx ≥ 4` (w=1) or `lineIdx ≥ 8` (w=2) — not the intended
"outside the 16-cell grid" test. This biases all line pieces toward the left of the
lattice.

### 4.3 Result

`Puzzle = Shape[][]`, length 9, many entries empty. Empty partitions are `continue`d in
`puzzleLayers`, so **the layer/color index `i` advances only over non-empty
partitions**, while the lattice anchor `idx` advances over all 9. Typical output: 3–6
non-empty layers.

---

## 5. Per-partition field: the smooth union (`scene.ts`)

Each non-empty partition becomes one GLSL function `sceneN(vec2 uv) -> float`, a
sequential fold over its shapes:

```glsl
float sceneN(vec2 uv) {
    float scene = 1000.0;
    float a = 0.0, b = 0.0, k = 0.0, m = 0.0, h = 0.0, s = 0.0;
    k = 0.155;                       // <-- THE smooth-union factor

    // repeated once per shape in the partition:
    a = rectSDFj(uv);
    b = scene;
    h = max(k - abs(a - b), 0.0) / k;
    m = pow(h, 1.55) * 0.5;
    s = m * k / 1.55;
    if (a < scene) { scene = a - s; } else { scene = b - s; }

    return scene;
}
```

### 5.1 The smooth-min in closed form

```
smin_k(a, b) = min(a, b) − (k / 1.55) · 0.5 · ( max(k − |a − b|, 0) / k )^1.55
```

with **k = 0.155** and **exponent p = 1.55** (the divisor is the same 1.55). This is a
variant of iq's polynomial smooth-min, which conventionally uses `h²·0.5` and `k/2`;
here the exponent and the divisor are both retuned to 1.55, giving a slightly *sharper*
falloff than quadratic and a maximum blend depth of

```
s_max = 0.5 · k / 1.55 = 0.5 · 0.155 / 1.55 = 0.05
```

reached when `a == b`. So the fillet pulls the isosurface outward by at most **0.05 uv
units** — the visually critical number for the "melted together" look. The blend has
support only where `|a − b| < 0.155`.

### 5.2 Fold semantics and order dependence

- Init `scene = 1000.0`; for the first shape `|a−b| ≫ k` ⇒ `h = 0, s = 0` ⇒
  `scene = a`. Correct no-op.
- The fold is **sequential and order-dependent** (blending against the accumulated
  field, not pairwise) — a shape blends with the *already-blended* result, which
  slightly deepens fillets in chains of 3+ cells. Reproduce the fold order (partition
  order = `getPiece` offset order) to match output exactly.
- `union` (plain `min`) exists in `scene.ts` but the glyph path always passes
  `{type: "SMOOTH_UNION", factor: 0.155}`.

---

## 6. Per-layer shading (`puzzle.ts::puzzleLayers`, `2d-lighting.ts`)

For layer `i` (0-based over **non-empty** partitions):

### 6.1 The height field used for normals

```glsl
float sceneNsmoothed(vec2 uv) {
    vec3 p = vec3(uv.x, uv.y, 0.4) * 0.8;
    float per = (1.0 * perlin(p));            // COMPUTED BUT UNUSED (dead code)
    return 1.8 * pow(smoothstep(-0.15, 0.1481753576592919866, sceneN(uv)), 8.0);
}
```

Constants: amplitude **1.8**, smoothstep edges **−0.15 → 0.1481753576592919866**,
exponent **8.0**.

Behaviour: the field is 0 deep inside the blob (`sdf < −0.15`), rises to 1.8 outside
(`sdf > 0.148`), and at the boundary (`sdf = 0`) sits at
`1.8·(0.15/0.2981…)^8 ≈ 1.8·0.0039 ≈ 0.007`. The `pow(·,8)` crushes the ramp toward the
outer edge, so **the gradient is essentially zero across the blob interior and spikes in
a thin band at/just inside the rim** — which is exactly what produces the flat-lit body
with a soft beveled edge. The perlin term is vestigial; keeping the `vec3 p` line is
harmless but it contributes nothing.

### 6.2 Normal estimation (`estimateNormal2D`)

```
EPSILON = 0.0001
N = normalize( vec3(
      F(uv.x + ε, uv.y) − F(uv.x − ε, uv.y),
      F(uv.x, uv.y + ε) − F(uv.x, uv.y − ε),
      2ε ) )
```

where `F = sceneNsmoothed`. Note the differences are **not divided by 2ε** — the raw
central differences are used directly against a z-term of `2ε = 0.0002`. That fixed
ratio is what sets the apparent "steepness" of the bevel: flat regions (differences ≈ 0)
give `N ≈ +z`, rim regions give a strongly tilted normal. Any reimplementation must
keep the un-normalized differencing *and* the same ε, or the bevel contrast changes.

Each layer gets its **own** normal from its **own** partition field — that is the point
of partitioning ("so the lighting doesn't get confused"); layers therefore read as
separate physical slabs even where they overlap.

### 6.3 Lights (exact vectors, unnormalized)

```
L1 = vec3(-0.11,      -0.8138,      0.3)
L2 = vec3(-0.5238,     0.3,         1.4)

diffuse1 = 0.293913139      * dot(L1, N)
diffuse2 = 0.59951515132689 * dot(L2, N)

viewerDir = vec3(uv.x, uv.y, 1.0) − vec3(-0.81891595195, 1.3915939391, 0.874419191918)
H1 = normalize(L1 + viewerDir)
H2 = normalize(L2 + viewerDir)

spec1 = pow(0.99  * dot(N, H1), 24.0)
spec2 = pow(0.969 * dot(N, H2), 22.0)
```

`viewerDir` is position-dependent, so the highlight sweeps across the canvas (a cheap
fake of a nearby eye point at `(-0.819, 1.392, 0.874)`). Lights are deliberately not
normalized — their magnitudes (`|L1| ≈ 0.874`, `|L2| ≈ 1.52`) are folded into the
intensity.

Caveat: `dot(N, H)` can be negative, and `pow(negative, 24.0)` is undefined in GLSL
(drivers typically return 0 or NaN). The original relies on this; a port should clamp
with `max(dot(N,H), 0.0)` and verify it doesn't change the look, or replicate the
clamp-to-zero behaviour.

### 6.4 Layer color — including the operator-precedence artifact

The TypeScript builds

```ts
color = mult(add(mult(0.51513593, normalHalfway), dotted1, dotted2), colors[i % 3])
```

but the codegen (`math.ts::op`) joins terms **without parentheses**, so the emitted GLSL
is literally:

```glsl
0.51513593*pow(0.99*dot(N,H1),24.0) + pow(0.969*dot(N,H2),22.0)
  + 0.293913139*dot(L1,N)
  + 0.59951515132689*dot(L2,N) * vec4(r, g, b, 1.0)
```

Under GLSL precedence this evaluates as

```
layerColor = vec4( S ) + 0.59951515132689 * dot(L2, N) * vec4(r, g, b, 1.0)

where  S = 0.51513593 * spec1  +  spec2  +  0.293913139 * dot(L1, N)
```

Two consequences that are load-bearing for the look, both accidental:

1. **Only the L2 diffuse term is tinted.** The L1 diffuse and both speculars are added
   as an *achromatic* (white) term `S` broadcast across all four components — this is
   why highlights blow out to white/desaturate rather than staying in-hue.
2. **The 0.51513593 scale applies only to `spec1`**, because `normalHalfway =
   spec1 + spec2` is likewise unparenthesized. `spec2` enters at full strength.
3. **Alpha is lit too.** `S` and the tint term both hit the `.w` component, so
   `gl_FragColor.a` varies with the lighting — opacity is not constant across a blob.

Color selection is `colors[i % colors.length]` with 3 colors, so layers cycle
green→pink→blend→green… (after the reversal in §6.5).

### 6.5 Intersection color, and the reversal no-op

```ts
let intersectionColors = colors.reverse();   // reverse() mutates IN PLACE and returns the same array
```

`intersectionColors` is the **same array object** as `colors`, so after this line
`colors[i] === intersectionColors[i]` for all i. The intent was clearly "use the
reversed palette for creases"; what actually happens is:

- the palette is reversed in place (so layer 0 gets what was color 2, etc.), **and**
- the crease color equals the layer color, differing **only** in the specular scale:
  `0.321513593` instead of `0.51513593`.

```glsl
intersectionColor = 0.321513593*spec1 + spec2 + 0.293913139*dot(L1,N)
                  + 0.59951515132689*dot(L2,N) * vec4(same r,g,b,1.0)
```

Because `puzzleLayers` runs inside a React `useEffect` keyed on `[colors, algo]`, a
re-run reverses the array *again* — palette order can flip between renders. Another
non-determinism source to eliminate in a port.

### 6.6 Layer record

```ts
{ sdf,                    // sceneN(uv) fragment
  blur: 0.020,
  size: 0,                // unused
  color,                  // §6.4
  intersect: (i % 3) > 0, // layers 1,2,4,5,7,8 crease; 0,3,6 do not
  intersectionColor }     // §6.5
```

---

## 7. Compositing (`layers.ts::generateLayers`)

Background: `vec4(0.0, 0.0, 0.0, 0.0491)` — black at ~5% alpha (canvas shows through as
black in the reference renders).

```
color       = background
unionSoFar  = 10000000.0

for i, layer in layers:                       # back-to-front painter's algorithm
    color = mix(layer.color, color, smoothstep(0.0, 0.020, layer.sdf))

    if i > 0 and layer.intersect:
        intersectionSDF = max(unionSoFar, layer.sdf + 0.05)
        color = mix(layer.intersectionColor, color,
                    smoothstep(0.0, 0.020, intersectionSDF + 0.001))

    unionSoFar = min(unionSoFar, layer.sdf)

gl_FragColor = color
```

Details:

- **Coverage**: `smoothstep(0, blur, sdf)` is 0 inside (`sdf ≤ 0`) → take the layer
  color, 1 outside (`sdf ≥ 0.020`) → keep what's underneath. `blur = 0.020` uv units is
  the entire antialiasing/soft-edge budget; at 400 px with 1 uv = 200 px that's ~4 px.
- **Crease pass**: `max(unionSoFar, sdf + 0.05)` is the intersection of *everything
  already drawn* with the current blob **eroded by 0.05** (adding 0.05 to an SDF shrinks
  the shape). So the crease band is drawn 0.05 uv units inside the current layer's
  boundary wherever it overlaps a previous layer — the recessed lip / drop-shadow seam
  visible where blobs cross. The extra `+ 0.001` inside the smoothstep is a hairline
  bias.
- Note the crease is painted **in the current layer's own color** (§6.5), only with a
  weaker specular — so overlaps read as a darker, matte version of the same material
  rather than a different hue.
- `unionSoFar` accumulates a plain (hard) `min`, not a smooth min.
- Layers 0, 3, 6 (`i % 3 == 0`) never draw creases.

---

## 8. Complete generated shader (shape of it)

`shader.ts::boilerplate` wraps everything:

```glsl
precision mediump float;
uniform float width;
uniform float height;
// (no other uniforms are used by the glyph path)

float rand(vec2);                      // unused by glyph
vec3 mod289(vec3); vec4 mod289_4(vec4); vec4 permute(vec4);
vec4 taylorInvSqrt(vec4); vec3 fade(vec3);
float perlin(vec3);                    // referenced by *smoothed but result discarded
float random(vec2);                    // unused
vec3 hsl2rgb(vec3); vec3 rgb2hsl(vec3); vec4 hueshift(vec4,float);   // unused
const mat3 identity; mat2 rotate2D(float);                            // unused

// emitted per shape:   float [prefix]rectSDF0(vec2 p) { ... }
// emitted per layer:   float scene0(vec2 uv) { ... smooth-union fold ... }
//                      float scene0smoothed(vec2 uv) { ... }

void main() {
    vec2 uv = (1.0*gl_FragCoord.xy - vec2(width, height))/height;
    gl_FragColor = <one giant inlined expression>;
}
```

Everything after `uv` is a single expression: the normal estimation is *not* hoisted
into variables, so `sceneNsmoothed` is invoked **4×** per layer per pixel (central
differences), and the whole normal expression is textually re-expanded for each of
`dotted1`, `dotted2`, `spec1`, `spec2`, and again for the intersection color. A layer
with a 5-cell piece therefore evaluates its 5 rounded-box SDFs on the order of dozens of
times per pixel. Any port should CSE this aggressively (compute `N` once per layer per
pixel); it is a codegen artifact, not part of the visual definition.

Shape function names are prefixed `"yo"` (`SDF("yo")` in `puzzleLayers`) and numbered
globally via a closure counter — a single `SDFGenerator` is shared across all layers, so
numbering is unique across the whole shader, and identical shapes appearing in two
partitions get two separate functions.

---

## 9. Exact constant inventory

Everything a reimplementation must match:

| Constant | Value | Where |
| --- | --- | --- |
| lattice | 4 × 4 | `usePuzzle.ts` |
| base half-extent | 0.18 (× 0.18) | `usePuzzle.ts` |
| corner radius | 0.18 (⇒ circle) | `usePuzzle.ts` |
| spacing multipliers | x 1.04, y 1.02 | `usePuzzle.ts` |
| stagger | x 1.0 (odd rows +0.18), y 0 | `usePuzzle.ts` |
| lattice step | x 0.3672, y 0.3636 | derived |
| lattice origin | x −0.6444, y −0.5472 | derived |
| **smooth-union k** | **0.155** | `usePuzzle.ts` |
| **smooth-union exponent / divisor** | **1.55 / 1.55**, with ×0.5 | `scene.ts` |
| max fillet depth | 0.05 | derived |
| scene init | 1000.0 | `scene.ts` |
| height-field amplitude | 1.8 | `puzzle.ts` |
| height-field smoothstep | −0.15 → 0.1481753576592919866 | `puzzle.ts` |
| height-field exponent | 8.0 | `puzzle.ts` |
| normal ε | 0.0001 (z term = 2ε, undivided) | `2d-lighting.ts` |
| light 1 | (−0.11, −0.8138, 0.3), gain 0.293913139 | `puzzle.ts` |
| light 2 | (−0.5238, 0.3, 1.4), gain 0.59951515132689 | `puzzle.ts` |
| eye point | (−0.81891595195, 1.3915939391, 0.874419191918) | `puzzle.ts` |
| specular 1 | ×0.99, exp 24 | `puzzle.ts` |
| specular 2 | ×0.969, exp 22 | `puzzle.ts` |
| specular scale (body) | 0.51513593 (applies to spec1 only) | `puzzle.ts` |
| specular scale (crease) | 0.321513593 (spec1 only) | `puzzle.ts` |
| layer blur | 0.020 | `puzzle.ts` |
| crease erosion | 0.05, plus 0.001 bias | `layers.ts` |
| crease rule | `i > 0 && i % 3 > 0` | `puzzle.ts` |
| background | vec4(0, 0, 0, 0.0491) | `usePuzzle.ts` |
| palette gains | ×1.6, ×1.5, ×1.5, clamped to 1 | `GenerativeGraphic.tsx` |
| color-2 blend | 0.6/0.4, 0.6/0.4, 0.8/0.2 vs color 1 | `GenerativeGraphic.tsx` |
| seed quantization | byte % 19, 9 slots | `GenerativeGraphic.tsx` |
| `-1` injection | ≤4 slots, p = 0.10 each | `GenerativeGraphic.tsx` |
| canvas | 400 × 400, resolution 1 | `GraphicComponent.tsx` |

---

## 10. Faithful-port checklist

Behaviours that are accidents of the original implementation. Each must be either
replicated (to match the reference renders pixel-for-pixel) or **consciously** changed:

1. **uv window is `[-1,0]²`, not centered** (§1) — cells with positive coords are cropped.
2. **Unparenthesized color expression** (§6.4) — L1 diffuse + both speculars are white
   and untinted; `0.51513593` scales only `spec1`; alpha is lighting-dependent.
3. **`colors.reverse()` aliasing** (§6.5) — crease color == layer color; palette order
   flips on effect re-run.
4. **Random `-1` injection** (§3.1) — the algorithm is not a pure function of the seed.
5. **Two independent seed draws** for colors vs pieces (§3.3).
6. **Shadowed `width`/`idx` in the line branch** (§4.2) — line pieces are dropped by an
   unintended bound.
7. **Dead perlin evaluation** in the height field (§6.1).
8. **`pow` of a possibly-negative dot product** (§6.3).
9. **Central differences not divided by 2ε** (§6.2) — the bevel steepness depends on it.
10. **Sequential (not pairwise) smooth-union fold** (§5.2) — order-dependent fillets.
11. **`max(...)` with >2 args in `math.ts` delegates to `min`** — a latent bug; not hit
    by the glyph path (only 2-arg `max` is used), but don't copy the helper as-is.
12. Massive expression duplication (§8) — safe to CSE; purely a performance artifact.

---

## 11. Minimal reference reimplementation (pseudocode)

Self-contained restatement, CSE'd, with the §10 accidents preserved:

```
render(seed, W, H):
    nums_c  = seedNumbers(seed)            # 9 × [0..18] ∪ {-1}
    nums_a  = seedNumbers(seed)
    palette = paletteFrom(nums_c)          # 3 RGB, §3.2
    palette.reverse()                      # §6.5
    lattice = grid4x4()                    # §2.2, circles r=0.18
    parts   = [ partitionFor(nums_a[i], i, lattice) for i in 0..8 ]   # §4
    parts   = [ p for p in parts if p != [] ]

    for each pixel (px, py):
        uv    = (vec2(px, py) - vec2(W, H)) / H
        color = vec4(0, 0, 0, 0.0491)
        unionSoFar = 1e7
        for i, part in enumerate(parts):
            d  = sceneSDF(part, uv)                       # §5 fold, k=0.155
            N  = normalOf(part, uv)                       # §6.1–6.2
            base = palette[i % 3]
            S    = 0.51513593*spec1(N,uv) + spec2(N,uv) + 0.293913139*dot(L1,N)
            lay  = vec4(S) + 0.59951515132689*dot(L2,N) * base
            color = mix(lay, color, smoothstep(0, 0.020, d))
            if i > 0 and i % 3 > 0:
                Sc  = 0.321513593*spec1(N,uv) + spec2(N,uv) + 0.293913139*dot(L1,N)
                cr  = vec4(Sc) + 0.59951515132689*dot(L2,N) * base
                isd = max(unionSoFar, d + 0.05)
                color = mix(cr, color, smoothstep(0, 0.020, isd + 0.001))
            unionSoFar = min(unionSoFar, d)
        emit color

sceneSDF(shapes, uv):
    k = 0.155; scene = 1000.0
    for s in shapes:
        a = roundedBox(uv - s.center, vec2(s.hw, 0.18), 0.18)
        h = max(k - abs(a - scene), 0.0) / k
        off = pow(h, 1.55) * 0.5 * k / 1.55
        scene = min(a, scene) - off
    return scene

heightField(shapes, uv) = 1.8 * pow(smoothstep(-0.15, 0.1481753576592919866,
                                               sceneSDF(shapes, uv)), 8.0)

normalOf(shapes, uv):
    e = 0.0001
    return normalize(vec3(
        heightField(shapes, uv + vec2(e,0)) - heightField(shapes, uv - vec2(e,0)),
        heightField(shapes, uv + vec2(0,e)) - heightField(shapes, uv - vec2(0,e)),
        2*e))
```

---

## 12. Which knobs actually change the look

Ranked by visual leverage, for whatever drives it later:

1. **Which lattice cells are occupied, and how they group into partitions** — the silhouette. This is the piece table + anchor offset (§4).
2. **Number of partitions** — how many separately-lit, separately-colored slabs stack up, and therefore how many creases appear (§6, §7).
3. **Palette (3 colors) and layer→color assignment `i % 3`** (§3.2, §6.4).
4. **k = 0.155 / exponent 1.55** — how molten vs. beaded the union reads (§5).
5. **Crease erosion 0.05 and the `i % 3 > 0` rule** — depth/frequency of the seams (§7).
6. **Height-field smoothstep window and exponent 8** — bevel width; larger window or lower exponent = fatter, softer, more "inflated" shading (§6.1).
7. **Light vectors / eye point** — overall material mood; low leverage on identity, high on perceived polish (§6.3).
8. Lattice spacing/stagger — whether cells fuse at all. At step 0.3672 with radius
   0.18 the discs are 0.0072 apart, i.e. essentially tangent, and the fillet bridges
   them fully. The blend has support only while `|a − b| < k = 0.155`, so once the
   surface-to-surface gap approaches ~0.155 (center distance ~0.515, a spacing
   multiplier of ~1.86) neighbours stop bridging and the glyph falls apart into
   separate beads.
