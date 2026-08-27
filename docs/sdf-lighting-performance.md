# Generated SDF lighting cost

`eseqlisp_sdf_lighting_probe` renders a 1920×1080 headless control panel made
of 96 generated SDF controls. The representative material uses the rounded
control field and normal/specular-dependent color structure used by
`content/ui/materials.lisp`. Each sample submits one frame and waits for GPU
completion; pipeline creation and 30 warm-up frames are outside the measured
loop.

Run the probe with:

```sh
cargo run --release -p eseqlisp --features wgpu,capture-harness \
  --bin eseqlisp_sdf_lighting_probe
```

## Intel UHD 620 result

Measured on 2026-08-24 on an Intel UHD Graphics 620 (Kaby Lake GT2), Vulkan,
with 120 frames per tier:

| Generated lighting | Median | Mean | p95 |
|---|---:|---:|---:|
| Full finite-difference lighting | 3.408 ms | 3.405 ms | 3.672 ms |
| Flat quality tier | 2.714 ms | 2.726 ms | 3.021 ms |

Flat lighting reduced median GPU-completion wall time by **20.4%** (1.26×).
The probe reports host-observed completed-frame time rather than timestamp-query
shader time, so both rows include the same command submission and render-pass
overhead.

## Apple M1 Max result

Measured on 2026-08-25 on an Apple M1 Max (24-core GPU, macOS 26.5.1), wgpu on
the Metal backend, same probe and same 120 frames per tier:

| Generated lighting | Median | Mean | p95 |
|---|---:|---:|---:|
| Full finite-difference lighting | 1.293 ms | 1.304 ms | 1.364 ms |
| Flat quality tier | 1.281 ms | 1.274 ms | 1.316 ms |

Flat lighting reduced median wall time by **0.9%**, which is not a real
difference: a second run of the same binary put flat 0.7% *slower* than full.
The two tiers straddle each other from run to run, and both bottom out at the
same ~1.04 ms minimum, so what this probe measures on this GPU is submission and
completion overhead rather than fragment ALU. The lighting term is below the
noise floor.

That is the point of pairing the two rows:

| | Full | Flat | Median reduction |
|---|---:|---:|---:|
| Apple M1 Max (Metal) | 1.293 ms | 1.281 ms | 0.9% (noise) |
| Intel UHD 620 (Vulkan) | 3.408 ms | 2.714 ms | 20.4% |

Five field evaluations per pixel cost nothing measurable on Apple silicon and
about a fifth of the panel's GPU time on a Kaby Lake-R iGPU. The flat tier
therefore earns its keep only as a low-end fallback, and enabling it on Apple
hardware buys nothing — which is why full lighting stays the default rather than
being chosen per platform.

## Mitigation

The generated-shader API now has a shared `SdfShaderOptions` quality policy for
both the Metal and WGSL emitters. Full lighting remains the default. Flat
lighting keeps the authored material color expression and supplies neutral
bindings (`normal = (0, 0, 1)`, `diffuse = 1`, `specular = 0`) without emitting
any offset field samples, normal/light normalization, diffuse/specular dot
products, or specular `pow` from the lighting model. Authored color expressions
still run unchanged.

Applications can select the low-end fallback at process startup:

```sh
ESEQ_SDF_LIGHTING_QUALITY=flat <application command>
```

Accepted values are `full` and `flat`; invalid values fail generated shader
compilation with a configuration error. The material shader cache includes the
quality policy, so shaders from different tiers cannot alias.
