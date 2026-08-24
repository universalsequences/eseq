# Generated SDF lighting cost

`eseqlisp_sdf_lighting_probe` renders a 1920×1080 headless control panel made
of 96 generated SDF controls. The representative material uses the rounded
control field and normal/specular-dependent color structure used by
`content/ui/materials.lisp`. Each sample submits one frame and waits for GPU
completion; pipeline creation and 30 warm-up frames are outside the measured
loop.

Run the probe with:

```sh
cargo run --release -p eseqlisp --features wgpu \
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
