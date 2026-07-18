# GPU Element Displacement Engine — spec v1

Compute-pass vertex displacement over the shared upload arena: melt/rebuild,
audio-reactive "particles on a speaker" modes, per-element resonance, and a
demo-mode control surface. The entire effect runs on the GPU; the CPU's only
per-frame contribution is a small uniform struct and (once, per frame) a range
table describing where each element's vertices live.

Everything here targets the Metal backend (`crates/eseqlisp/src/ui/metal_backend.rs`)
and the widget primitive layer (`crates/eseqlisp/src/widget_render/mod.rs`).

## Why this architecture (decision record)

The renderer computes all vertex positions on the CPU in NDC and uploads them
per frame through `GpuUploadArena` (metal_backend.rs ~1475). Every render
pipeline — text, prop text, image, cable, waveform, wavetable, spectrogram,
circle/rect SDF, and the runtime-compiled widget shaders — consumes those
pre-transformed positions with a pass-through vertex shader
(`out.position = float4(v.position, 0, 1)`).

Rejected alternatives:

- **CPU-side displacement at emission time.** Zero shader work, but it defeats
  primitive-run caching (`MetalPrimitiveRun.reused_from_previous`) — every
  frame of animation would force full re-emission and re-upload. It is also
  not the story we want to tell: the point is the GPU does this.
- **Per-pipeline vertex-shader melt.** Requires editing ~10 shader families
  plus the compiled-widget preamble, AND threading a per-vertex element-id
  attribute through every vertex struct layout on both CPU and GPU sides.
  Strictly more invasive than the compute pass for the same result.

Chosen: **one compute kernel, encoded before the render encoder in the same
command buffer, that displaces positions in-place in the upload arena.** No
render pipeline changes at all. The kernel is a pure function:

```
displacement = melt(t, element)            // demo melt/rebuild envelope
             + spectrum_field(pos)         // audio field sampled by screen position
             + resonance(element)          // per-element mass-spring state
             + trigger_env(t)              // sequenced/triggered impulses
```

Modes are weightings of these terms. New modes = new kernel code against the
same tables; nothing else changes. That contract (range table + element state
buffer stable from day one) is the load-bearing design decision.

## Existing seams this builds on (verified in code)

| Fact | Where |
|---|---|
| Arena: triple-buffered `StorageModeShared` buffer, 256-aligned slices via `upload_slice<T>` returning `UploadedBufferSlice { buffer, offset }` | metal_backend.rs ~1443–1550 |
| All vertex structs start with `position: [f32; 2]` in NDC (text `Vertex`, `ImageVertex`, etc.); stride varies per pipeline | metal_backend.rs ~101, ~172, ~1432 |
| Element identity already exists CPU-side: `MetalPrimitiveRun { widget_id, ordinal, ancestor_widget_ids }` and `MetalPrimitive::ZLayer { z_index }` | widget_render/mod.rs ~628–685 |
| Spectrum already on GPU: `LiveSpectrogramGpuResource { bins, time_slices, write_head, sample_rate, waterfall_buffer, smoothed_buffer }` keyed by name in `live_spectrogram_buffers: HashMap<String, _>` | metal_backend.rs ~1890, ~2267 |
| Time uniform already threaded: `viewport.time_seconds` refreshed via `refresh_widget_scene_time` | metal_backend.rs ~2578 |
| Two frame paths: the tiled path (drawable acquired ~4846, per-tile scissor loop ~4885) and a second path ~6657. Phase 1 hooks the tiled path only; the second path gets the identity fallback (kernel not encoded → nothing changes) | metal_backend.rs |
| No compute encoder exists anywhere yet — this adds the first `MTLComputePipelineState` / `computeCommandEncoder` usage | verified by grep |

## Core data structures

### 1. Displacement uniforms (CPU → GPU, per frame, ~128 bytes)

```rust
#[repr(C)]
struct DisplacementUniforms {
    time: f32,
    melt_amount: f32,          // 0 = identity (kernel not even encoded), 1 = full melt
    mode_weights: [f32; 4],    // melt, speaker, resonance, cymatics
    gravity: f32,
    chaos: f32,                // noise amplitude for per-element phase scatter
    stiffness: f32,            // resonator spring constant scale
    damping: f32,
    band_env: [f32; 4],        // low/mid-lo/mid-hi/high envelope followers (CPU-side, from audio tap)
    trigger_env: f32,          // decaying impulse (kick-triggered flinch etc.)
    spectrum_bins: u32,
    spectrum_write_head: u32,
    viewport_px: [f32; 2],
    seed: u32,
    _pad: [u32; 3],
}
```

`band_env` is the pragmatic CPU-side path for scalar reactivity (the audio tap
already exists); the *field* reactivity (per-position lookup) uses the GPU
spectrum buffer directly and never touches the CPU.

### 2. Range table (CPU → GPU, rebuilt each frame during emission)

One entry per **element** (not per draw call — a single `upload_slice` batch
may contain many elements' quads):

```rust
#[repr(C)]
struct DisplacementRange {
    byte_offset: u32,   // into the arena frame buffer, start of this element's vertices
    vertex_count: u32,
    stride: u32,        // bytes per vertex for this pipeline's layout
    pos_offset: u32,    // byte offset of position within the vertex (0 for all current layouts)
    element_slot: u32,  // index into the element state buffer (see §3)
    z_index: i32,
    pivot_ndc: [f32; 2],   // element center, for rigid tumble
    half_size_ndc: [f32; 2],
    flags: u32,         // bit 0: rigid (tumble as body) vs per-vertex field; bit 1: text glyph; ...
    _pad: u32,
}
```

**Recorder API.** A small `DisplacementRecorder` owned by the backend, active
only when `melt_amount > 0`:

- `recorder.begin_element(element_key, z_index, pivot, half_size, flags)` /
  `recorder.end_element(vertex_range)` — called where vertex `Vec`s are built,
  while the emitting code still knows which widget/glyph/quad it is working on.
  `element_key` is `(widget_id, ordinal, intra_ordinal)` hashed to a stable u64.
- Ranges are recorded in **local vertex indices** during construction, then
  fixed up to arena byte offsets right after the `upload_slice` call returns
  the real `(buffer, offset)`.
- When the recorder is inactive (normal operation) every call is a no-op and
  the render path is byte-for-byte identical to today. This keeps the feature
  zero-cost when off — non-negotiable.

Granularity guidance: a glyph run records per-glyph elements (text melts
per-character — this looks great and glyph quads are already fine-grained); a
knob or button records one element for its whole chassis; large filled rects
see §Subdivision.

### 3. Element state buffer (GPU-resident, persistent across frames)

For resonators. `MTLBuffer`, `StorageModePrivate`, `N_slots × ElementState`:

```
struct ElementState { float2 disp; float2 vel; float energy; uint last_seen_frame; }
```

CPU keeps `HashMap<u64 /*element_key*/, u32 /*slot*/>` with LRU slot reuse
(start with 16k slots, grow by reallocation + blit copy). Because the key is
derived from `widget_id`/ordinal, an element keeps its oscillator state across
frames even as its vertices are re-emitted — this is what makes elements
*ring* after a transient instead of merely tracking the spectrum.

### 4. Spectrum binding

Bind the designated global analyzer's `smoothed_buffer` (master bus). If no
live-spectrogram widget is mounted, displacement mode must ensure the master
analyzer runs anyway — reuse the same machinery that feeds
`live_spectrogram_buffers`, registered under a reserved name
(`"__displacement_master__"`) so a UI widget and the displacement field can
coexist or exist independently.

## The compute kernel

One MSL kernel, one dispatch, one thread per **vertex** (ranges flattened into
a prefix-sum index so thread i finds its range by binary search, or simpler:
dispatch per-range with `dispatchThreadgroups` per entry — start with the
simple per-range loop encoding, optimize later only if encoder overhead shows
up in `RenderStats`).

Pseudocode of the force terms:

```metal
kernel void displace(device uchar* arena,
                     device const DisplacementRange* ranges,
                     device ElementState* states,
                     device const float* spectrum,       // smoothed_buffer
                     constant DisplacementUniforms& u, ...)
{
    // resolve vertex pointer: arena + range.byte_offset + vid*stride + pos_offset
    float2 p = read_pos();
    float2 pivot = range.pivot_ndc;

    // per-element phase from z + hash(element_slot): peel off in z-order
    float phase = z_norm(range.z_index) + u.chaos * hash01(range.element_slot);
    float m = melt_envelope(u.melt_amount, phase);        // staggered onset

    // MELT (rigid): translate + tumble around pivot, gravity accelerates
    float2 rigid = rigid_fall(p, pivot, m, u.gravity, u.time, range.element_slot);

    // SPEAKER: x → frequency band, displace vertically by magnitude
    float band = mix(0.0, float(u.spectrum_bins - 1), screen_x01(p));
    float mag  = spectrum_lookup(spectrum, band, u.spectrum_write_head);
    float2 speaker = float2(0.0, mag * speaker_gain);

    // RESONANCE: one-pole mass-spring per element, driven by band_env at the
    // element's resonant frequency (freq ∝ 1/element area → panels wobble low,
    // glyphs jitter to hats). State read/modify/write by thread 0 of each
    // element (or precomputed in a tiny per-element pre-pass — see note).
    float2 res = states[range.element_slot].disp;

    // CYMATICS: standing-wave field of screen position × band amplitudes
    float2 cym = chladni(p, u.band_env) * cymatics_gain;

    float2 d = u.mode_weights[0]*rigid + u.mode_weights[1]*speaker
             + u.mode_weights[2]*res   + u.mode_weights[3]*cym
             + u.trigger_env * flinch(p, range.element_slot);
    write_pos(p + d);
}
```

Resonator integration note: mass-spring state updates must happen once per
element per frame, not once per vertex. Encode a tiny **pre-pass dispatch**
(one thread per range entry) that integrates `ElementState`, then the main
per-vertex pass reads the result. Two dispatches, same encoder, trivial cost.

**Rebuild is free:** every term is scaled by its weight/envelope; lerp them to
zero and the arena contents are bit-identical to what the CPU uploaded. No
"snap back" logic exists — identity is just the zero of the same function.

## Frame integration (tiled path)

```
collect primitives (recorder active if melt armed)
upload vertex slices  ──►  fix up range table offsets
upload ranges + uniforms (via upload_arena, same as any slice)
[if melt_amount > 0]:
    computeCommandEncoder
        pre-pass: integrate ElementState   (1 thread / range)
        main:     displace vertices        (1 thread / vertex)
    endEncoding
renderCommandEncoderWithDescriptor(...)   // existing, untouched
    ... existing per-tile scissor loop, all pipelines unchanged ...
```

Same command buffer, compute encoded before render — Metal guarantees
ordering. The arena is `StorageModeShared` + write-combined: CPU writes,
GPU compute read-modify-writes, GPU render reads. The CPU never reads it back,
so write-combined stays valid. The arena's in-flight tracking
(`GpuUploadFrame.in_flight`) already prevents CPU reuse before the command
buffer completes; the compute pass rides inside that same command buffer, so
no new synchronization is needed.

## Known gotchas (decided up front)

1. **Scissor clipping.** Per-tile scissor rects are screen-space and do not
   move with displaced vertices, so melting content clips at tile edges.
   Phase 1 verdict: **accept it** — "contents melt within their pane" reads
   fine. A later `flags` bit can widen the scissor to the full viewport for
   full-screen demo shots (`demo_unclipped: bool` on the displacement state).
2. **SDF locals don't move.** Pipelines like `ImageVertex` carry
   `local_pos/half_size` for corner masks; displacing `position` only is
   correct for rigid per-element motion (local space rides along). Non-rigid
   per-vertex warps slightly distort SDF masks — acceptable, and rigid mode is
   the default for chassis elements anyway.
3. **4-vertex quads shear, they don't bend.** Text is naturally fine-grained;
   large filled rects (panel backgrounds) tumble rigidly. §Subdivision covers
   the droop upgrade; it is *not* Phase 1.
4. **Text bg cells travel with glyphs**, revealing the clear color (theme BG)
   behind — visually correct, no action needed.
5. **Cached primitive runs are position-stale by design** — irrelevant here,
   because displacement happens after upload each frame; the cache reuse path
   re-uploads cached vertex data through the same arena, so displaced frames
   never poison the cache. Verify this explicitly in Phase 1 (assert the
   cache stores CPU-side vertex vecs, not arena contents).
6. **Hit-testing during melt**: input keeps using undisplaced layout. Fine —
   displacement modes are visual; if a mode is ever left on during normal
   editing, mild speaker/resonance wobble around true positions is still
   usable. Full melt is a demo/performance state, not an editing state.

## Subdivision (droop upgrade, later phase)

When melt mode is armed, primitive emission for large rects/quads (area over a
threshold, e.g. > 120×120 px) emits an N×M grid of quads instead of one
(`N,M ≈ ceil(size/40px)`, capped). Each subdivided rect records **one**
element with `flags.rigid = 0`, so the kernel applies the per-vertex field and
the panel actually bends/droops. Bounded cost, only in melt mode, emission-
side only — no pipeline changes. Rounded-corner rects keep radius geometry in
the corner cells (or simply skip subdivision when `radius > 0` in v1).

## Control surface

### Lisp natives (Phase 5, `crates/sequencer/src/ui/natives.rs`)

```lisp
(displacement-mode! :melt)        ; :off :melt :speaker :resonance :cymatics :custom
(displacement-param! :amount 0.8) ; :amount :gravity :chaos :stiffness :damping :weights
(displacement-trigger! 1.0)       ; impulse into trigger_env (decays in-kernel-visible uniform)
```

State lives in UI state values (`ui/state_values.rs` pattern), written into
`DisplacementUniforms` each frame. Being lisp-native means demo scripts are
just `.lisp` files, and hot-reload applies.

### Sequencer integration (optional, after Phase 5)

Add `ParamTarget::DisplacementParam { param: String }` alongside the existing
variants (`crates/sequencer/src/process.rs:337`) so macros, p-locks, and
processes can drive `:amount` etc. per step. This rides the same override seam
as the macro-mapping work (MACRO_MAPPING_SPEC.md); it is deliberately last —
everything before it works with natives alone.

### Audio feeds

- `band_env[4]` + `trigger_env`: computed from the existing audio tap
  (`src/audio_tap.rs` / `ui/live_audio_analyzer.rs`) — 4 one-pole envelope
  followers on band energies, written into uniforms. Trigger can also be fed
  from note-on events (kick track → flinch).
- Spectrum field: GPU buffer binding, §4 above. No CPU involvement.

## Phases

**Phase 0 — plumbing & arming (small).**
Uniform struct, backend fields (`displacement: Option<DisplacementCtx>`),
compute pipeline compilation (kernel source as `const` string like every other
shader in the file), a debug keybinding / native to arm it. Kernel body:
whole-screen sine wobble applied to *every* arena byte range covering vertex
slices — no range table yet; instead Phase 0 records just `(offset, len,
stride, pos_offset)` per `upload_slice` callsite that uploads vertices.
**Exit criterion:** the entire UI (text, knobs, cables, meters, compiled
widgets) wobbles as one sheet at 120fps with zero per-frame CPU cost beyond
the uniform write, and `melt_amount = 0` is verified byte-identical (capture
comparison via `ui/capture.rs`).

**Phase 1 — range table + recorder.**
`DisplacementRecorder`, element keys from `widget_id/ordinal`, pivot/half-size
capture, offset fix-up after `upload_slice`. Kernel switches to per-range
dispatch. Verify recorder-inactive path changes nothing (goldens again) and
measure recorder-active emission overhead in `RenderStats`.

**Phase 2 — melt/rebuild.**
Rigid fall + tumble + z-staggered onset + melt envelope; rebuild by ramping
`melt_amount` down. Tune `gravity/chaos` defaults. This is the first
tweetable output.

**Phase 3 — speaker mode.**
Global analyzer registration (`"__displacement_master__"`), spectrum buffer
binding, x→band vertical displacement. Second tweetable output; verify with
the audition harness playing a known sweep (left-to-right ripple should track
frequency).

**Phase 4 — resonance.**
Element state buffer + slot allocator + pre-pass integrator. Resonant freq
from element area. Tune so panels wobble ~2–6 Hz, glyphs 10–20 Hz.

**Phase 5 — modes, cymatics, lisp control surface.**
Mode weights, natives, demo scripts in `scripts/` (e.g.
`scripts/displacement-demo.lisp` sequencing: normal → melt → rebuild →
speaker → cymatics, synced to a project).

**Phase 6 (optional) — subdivision droop, ParamTarget variant, unclipped
demo scissor.**

## Testing & verification

- **Identity goldens:** `ui/capture.rs` screenshot comparison with
  displacement disarmed, before/after every phase — the recorder and kernel
  must be provably invisible when off.
- **Determinism harness:** armed with fixed `seed`, fixed `time` uniform →
  capture must be reproducible frame-for-frame (kernel uses only uniform
  inputs + hashes, no clock reads).
- **Perf:** `RenderStats` counters for recorder time, range count, compute
  encoder time (via command-buffer GPU timestamps). Budget: recorder < 0.2ms
  CPU at full UI; compute pass < 0.3ms GPU for ~100k vertices.
- **The UI-script test pattern** (see band-coupling memory) drives arming via
  natives in a scripted session for regression.

## Non-goals (v1)

- Displacing the TUI path or the second (non-tiled) render path.
- Hit-testing through displaced geometry.
- Bending single-quad elements without subdivision.
- The offscreen-composite effects (tiles-in-3D-space, Exposé buffer view,
  pixel-space fluid ripple) — separate infrastructure (render-to-texture +
  composite pass), separate spec. The two compose: element displacement runs
  first, composite warps the result. Nothing here precludes it.
