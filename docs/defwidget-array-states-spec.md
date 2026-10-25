# defwidget Array States & Live-Audio Data — Spec

Status: draft, 2026-10-18. Design from a chat session with Alec. Partly
built 2026-10-25 for mesh widgets only (docs/defwidget-vertex-stage.md):
`(name :floats n)` entries and the clamped `(name i)` read, fed from a Lisp
list per instance; rank-2, `f32-array`, snapshots and `:source` are not.
Companion concepts: `defwidget` scalar `:state` (existing,
`crates/eseqlisp/src/lang/vm/widget_state.rs`), the SDF shader DSL →
Metal/WGSL lowering (`crates/eseqlisp/src/lang/sdf_codegen.rs`), and the
live-audio tap pipeline (`crates/eseqlisp/src/widget_render/live_audio.rs`,
`widget_render/spectrogram.rs`).

## 1. Problem

`defwidget` shaders today receive only scalar state: each `:state` symbol
becomes one float uniform (or a small fixed pack of floats for vec3/instance
fields). There is no way to hand a widget shader *list-shaped* data — a row
of levels, a spectrum, a matrix — even though the two backend targets
(Metal, WGSL) both have first-class buffer/texture reads in fragment
shaders, and the webgl tradition solves exactly this by packing data into
textures.

Two use cases drive the design:

1. **User-supplied arrays**: a Lisp-computed list of numbers drawn as a
   matrix / bar grid / heatmap (the simple case).
2. **Live audio analysis in user space**: the built-in `spectrogram` /
   oscilloscope widgets do everything for you via the tap pipeline. Users
   want to build *their own* visualizers: raw spectrogram data from a track
   → Lisp math → array → shader. This must be efficient enough to run at
   repaint rate without allocation churn.

## 2. Design principles

- **Expose indexing, not textures.** The DSL stays at its current
  abstraction level: `:state` declares data, the shader body reads it,
  codegen picks the lowering. Whether an array rides in a uniform block, a
  storage buffer, or an R32F texture is a backend decision, never visible
  in Lisp.
- **Mirror the scalar model.** An array read is the same thing a scalar
  read already is — a symbol reference — extended from 0-arity to N-arity.
  Declared with its size, read by index, written whole.
- **One mechanism, three lowerings.** Metal: `constant float*` argument.
  WGSL: `var<storage, read> array<f32>`. WebGL (if ever revisited): R32F
  texture + `texture2D` lookup — which is exactly why the DSL must not
  mention textures.
- **Repaint-rate pull, never audio-rate push.** Nothing crosses the VM
  boundary per audio block. All live data flows at widget repaint cadence.
- **Whole-array operations only.** Arrays are transformed wholesale by
  natives, never walked element-by-element in interpreted Lisp.

## 3. State declaration syntax

`:state` entries generalize from bare symbols to typed entries:

```lisp
(defwidget level-grid
  :width 8 :height 8 :animates true
  :state (selected                        ; existing scalar form, unchanged
          (levels :floats 64)             ; rank-1: length 64
          (spec :floats 64 128))          ; rank-2: 64 rows × 128 cols
  :shader ...)
```

- `(name :floats n)` — rank-1, `n` elements.
- `(name :floats rows cols)` — rank-2. Sugar over a flat `rows*cols`
  buffer; the accessor arity must match the declared rank (compile error on
  mismatch).
- No rank-N for now. Flat + rank-2 covers the known cases; add more when a
  second real widget needs it.

Scalar planning (`widget_state.rs`) is untouched: array entries become a
new plan variant alongside `Scalar`, and the existing uniform float budget
(`budget_error`) keeps counting only scalars. Arrays are **always** lowered
to buffer/texture, never packed into the uniform block, regardless of size
— explicit over auto-promotion, so the ABI doesn't cliff at a size
threshold.

## 4. Shader-side access

One new form: applying an array symbol to index expression(s).

```lisp
(defwidget cell
  :width 8 :height 8
  :state ((levels :floats 8 8))
  :shader
  (sdf/fill (sdf/circle 0.3)
    (gray (levels (floor (* y 8)) (floor (* x 8))))))
```

`(levels row col)` reads `levels[row * 8 + col]`; `(levels i)` for rank-1
reads `levels[i]`. The array value is **opaque**: indexable, nothing else.
No `nth`, `map`, `length`, or other list operations — it is not a Lisp list
in the shader.

**Bounds.** Codegen emits a clamp (`min(i, len-1)`; texture sampling clamps
for free in a webgl lowering). Out-of-bounds must never become a Metal UB
read — those take down the whole UI.

**Lowering table:**

| Backend | Declaration | Read |
|---|---|---|
| Metal | `constant float* levels` (one `MTLBuffer` per array state per widget instance) | `levels[min(i, 63)]` |
| WGSL | `var<storage, read> levels: array<f32>` | `levels[min(i, 63u)]` |
| WebGL (future) | R32F texture + size uniforms | `texture2D(levels_tex, uv_of(i)).r` |

## 5. Lisp-side value: `f32-array`

The snapshot use case (§6) forces a real packed-float value type. Walking
Lisp lists and re-packing per frame at 60 Hz × thousands of floats is
allocation churn we can't afford, so:

- New primitive value `f32-array`: a packed `Vec<f32>` with length.
- Snapshot natives (§6) return `f32-array` directly — floats are boxed zero
  times from tap ring to GPU upload.
- Array states accept `f32-array` via `set-state` with a memcpy (or buffer
  swap) into the widget's upload cache. Whole-array replacement only in v1;
  no per-element `(set! (levels 3) x)` — keeps dirty-checking a single
  version bump / memcmp.
- Constructors/accessors for hand-built data: `(f32-array 1.0 2.0 …)`,
  `(make-f32-array n)`, `(a-ref a i)`, `(a-length a)`. `(a->list a)`
  exists explicitly so the allocation-heavy escape hatch is *visible* and
  owned by the caller.
- Accepting plain Lisp lists into array states is allowed for the static
  case (pack once at set time) but is not the live-data path.

**Whole-array transform natives** (the "Lisp math" affordability layer):

```lisp
(a-map! a f)            ;; in-place elementwise
(a-scale a k)
(a-smooth a alpha prev) ;; temporal smoothing against previous frame
(a-resample-log a n)    ;; log-frequency resample to n bins
```

Document the rule: arrays are transformed wholesale, never walked. Future
gravy: let `def-process` take/return f32-arrays so transforms JIT through
DGen — not required for v1.

## 6. Live audio: snapshot natives (tier 1)

The built-in `spectrogram` subscribes to an engine tap
(`LiveAudioSourceSelector` + `TapPoint` + FFT config); the engine maintains
a waterfall ring buffer and smoothed EQ buffer per tap, and the renderer
uploads slices to GPU buffers each frame. The FFT runs once per tap
regardless of consumer count. We expose that same infrastructure to Lisp as
a **read** API:

```lisp
(track-spectrum :track 3 :fft 2048)                  ; latest frame → f32-array
(track-spectrum-window :track 3 :fft 2048 :slices 64) ; 64×bins window → f32-array
(track-waveform :track 3 :n 512)                      ; oscilloscope → f32-array
```

Semantics:

- A snapshot **reads** the tap's existing ring; it never runs DSP itself.
  If the tap doesn't exist yet, declaring/reading the source creates it —
  same machinery, new consumer.
- `track-spectrum-window` is the scrolling-spectrogram answer: the tap ring
  *is* the history, returned as one flat f32-array shaped to drop directly
  into a rank-2 array state (`(spec :floats 64 128)` ↔ `:slices 64`,
  `fft/2`-ish bins resampled to 128). No Lisp-side ring of frames — that
  design is allocation city and is explicitly rejected.
- Copies are bounded and trivial: a frame is a few KB; a 64×128 window is
  32 KB. At repaint rate this is noise.

Full loop, user space:

```lisp
(defwidget my-scope
  :width 64 :height 16 :animates true
  :state ((spec :floats 64 128))
  :shader
  (sdf/fill (sdf/rounded-rect (* width 0.5) (* height 0.5) 0)
    (heat (spec (floor (* x 64)) (floor (* y 128))))))

;; per-frame refresh at repaint cadence:
(set-state! my-scope 'spec
  (a-resample-log (track-spectrum-window :track 3 :fft 2048 :slices 64) 128))
```

## 7. Sourced array states (tier 2)

For pass-through viz — raw spectrum in a shader, no Lisp math — tier 1
still charges a VM round-trip per frame. Tier 2 lets an array state declare
a **source** instead of being set:

```lisp
:state ((spec :floats 64 128 :source (spectrum :track 3 :fft 2048)))
```

The engine writes from the tap ring into the widget's upload buffer at
repaint time, exactly like the built-in spectrogram — the VM is never
involved. Same declaration surface, same shader accessor `(spec i j)`, two
feeds:

| Feed | Cost | Lisp transform? |
|---|---|---|
| `:source` on the state | zero VM cost, read-only | no |
| snapshot native + `set-state!` | one copy + transform per frame | yes |

Prototype in tier 1; flip to tier 2 when the transform turns out to be
identity.

## 8. Explicitly rejected

- **`:texture` state slot / handing pixel buffers from Lisp.** Leaks the
  webgl implementation strategy into the API, forces texel/RGBA thinking,
  and composes badly. Indexed read is the abstraction; texture is a backend
  detail.
- **List values inside the shader** (`(nth levels i)` on a real list).
  Codegen would have to understand list layout, and it invites
  non-lowering list ops. Opaque indexable value only.
- **Audio-block-rate callbacks into Lisp.** Wrong rate by ~3 orders of
  magnitude; repaint-rate pull is always sufficient for pixels.
- **Shader-side history** (previous-frame textures kept per widget). Forks
  the state model; the tap ring already is the history, in a form Lisp can
  also read. One source of truth.
- **Auto-promoting small arrays into the uniform block.** Predictable ABI
  beats saving a bind for tiny arrays.

## 9. Implementation outline

1. `lang/def_header.rs` / `lang/vm/widget_state.rs`: state entries become
   an enum — `Scalar(name)` (existing), `Array { name, shape }` where
   `shape = [n] | [rows, cols]`. `uniforms()` / float budget unchanged
   (scalars only); arrays get a parallel plan.
2. `lang/sdf_codegen.rs`: one new form, `(sym idx…)` for declared array
   `sym`, arity checked against rank at compile time; emits the clamped
   buffer/texture read per backend.
3. `f32-array` value type in the VM + constructor/accessor/transform
   natives (§5).
4. `ui/metal_backend.rs` / wgpu path: one buffer per array state per widget
   instance, uploaded from the packed cache on dirty; reuses the existing
   buffer pool / uploaded-slice machinery.
5. Live-audio snapshot natives (§6) on top of the existing tap registry.
6. Tier-2 `:source` plumbing (§7).

## 10. Acceptance sketch

- A `defwidget` with `(levels :floats 64)` renders a bar grid from a
  Lisp-set `f32-array`, on Metal and wgpu.
- Rank-2 shaped state + `track-spectrum-window` gives a working user-space
  scrolling spectrogram with no per-frame Lisp list allocation.
- OOB index in a shader is clamped, never a crash.
- Arity mismatch between accessor and declared rank is a compile error
  naming the widget and the state.
- Tier-2 sourced state renders the same spectrum with zero VM traffic per
  frame (verify by VM counters, not vibes).
