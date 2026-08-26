# Linux validation

What "the Linux port works" means, how it is checked, and what the numbers were
when it was last checked. Tracked by bead `eseq-linux.12`.

Reference machine for every Linux measurement below (the macOS host used for
the `eseq-linux.26` cross-checks is named where its numbers appear):

| | |
|---|---|
| CPU | Intel Core i7-8650U (Kaby Lake-R, 4C/8T) |
| GPU | Intel UHD Graphics 620 (Kaby Lake GT2), shared memory |
| Graphics stack | Vulkan, Mesa 26.1.7 (`vulkan-intel`) |
| Target | `x86_64-unknown-linux-gnu` |

This is deliberately a weak integrated GPU on a laptop CPU. The renderer was
tuned for Apple GPUs (see `UI_PERFORMANCE_TUNING.md`), and SDF-heavy fragment
shaders with per-widget instancing behave differently when memory bandwidth is
shared with the CPU.

## Supported build targets

Exactly two, and the set is enforced at compile time. `app_paths/mod.rs` selects
the bundled DGenLisp compiler filename and toolchain target from the build
target and ends the `cfg` chain with

```rust
compile_error!("no bundled DGenLisp compiler exists for this target");
```

so anything else fails the build with that message rather than picking a
host-incompatible binary at runtime:

| Target | Toolchain stage | Status |
|---|---|---|
| `aarch64-apple-darwin` | `arm64-apple-macos` | supported |
| `x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu` | supported |
| `x86_64-apple-darwin` | none | **not supported** |

`eseq-linux.22` recorded its deferred cross-check as `x86_64-apple-darwin`; that
was wrong. Confirmed on a macOS host on 2026-08-25 (`eseq-linux.26`):

- `cargo check --workspace --all-targets --target aarch64-apple-darwin` — clean.
- The same with `--features eseqlisp/wgpu` (the Linux CI check's feature set) —
  clean.
- `--target x86_64-apple-darwin` — 8 errors in `sequencer` (lib) and 17 in
  `sequencer` (lib test), all cascading from the `compile_error!` above through
  `DGENLISP_TOOL_FILENAME`, `DGEN_TOOLCHAIN_TARGET`,
  `DGEN_TOOLCHAIN_REQUIRED_FILES` and `CACHE_TARGET_TRIPLE`. That is the gate
  working, not a port gap.

Adding an Intel-Mac target would mean publishing an `x86_64-apple-macos`
DGenLisp distribution and toolchain archive first; nothing in the Rust tree
blocks it.

## Test suite

The validated command is:

```sh
cargo nextest run --workspace --features eseqlisp/wgpu
```

`RUST_MIN_STACK` is supplied by `.cargo/config.toml`; do not set it by hand.
Both `./scripts/fetch_dgenlisp.sh` and `./scripts/fetch_dgen_toolchain.sh` must
have been run in the checkout first — anything that compiles a DGen patch
hard-fails without them, and that failure accounts for every one of the 66
failures recorded in this bead's earlier attempt on 2026-08-24.

Baseline on the reference machine, 2026-08-25 at commit 7396708d: **4508 run,
4508 passed, 33 skipped**, in 534s debug.

Do not use the macOS workspace counts in `docs/test-suite-performance.md` as a
Linux expectation. The two platforms compile different sets of targets.

## Tests skipped on Linux

The rule this section exists to enforce: **no test is skipped because the host
is Linux.** A test is either run, or it is `#[ignore]`d for a reason that is
equally true on macOS.

`#[cfg(target_os = "macos")]` used to gate 36 test functions across
`editor/tests.rs`, `lib.rs`, `ui/layout.rs`, `widget_render/patcher/tests.rs`,
and `sequencer/src/ui/state_values/tests.rs`. None of them were testing Metal.
They were gated because the primitive IR they assert against
(`GpuPrimitive`, `WidgetInstance`, `WidgetViewport`, `collect_gpu_primitives`)
was itself macOS-gated before `eseq-linux.1` made it backend-neutral; the gates
outlived the reason. All 36 now run on both platforms, and
`eseq-linux.26` re-confirmed them on a macOS host — see below.

What remains:

| Skipped | Count | Reason |
|---|---|---|
| `crates/eseqlisp/tests/capture.rs` (whole file, `#![cfg(target_os = "macos")]`) | 3 | Drives the `eseqlisp_capture` binary, which renders through `MetalBackend`. There is no Metal on Linux. All three tests are also `#[ignore]`d on macOS — they write PNGs for visual inspection rather than asserting. The wgpu equivalent is `eseqlisp_shader_capture`, which does run here. |
| `#[ignore]`d tests | 33 | Manual probes and benchmarks, not platform-conditional. |

The 33 `#[ignore]`s break down as: release-mode UI performance probes on the
project-92 and pianohold fixtures (16), micro-benchmarks run with
`--ignored --nocapture` (7), compiler/runtime and DGen profiling harnesses (5),
two tests for a legacy step grid `ui/main.lisp` no longer loads, one test that
`chdir`s the whole process, one that needs a default audio output device, and
one that needs a sample WAV from the author's local library.

`cargo nextest run --workspace --features eseqlisp/wgpu` therefore reports
exactly 33 skipped on Linux, and every one of them would also be skipped on
macOS.

### Re-confirmed on macOS, 2026-08-25

`eseq-linux.26` ran the ungated set on an Apple M1 Max (macOS 26.5.1) to check
that none of the 36 gates had been hiding a real failure rather than an obsolete
one. All 35 gate-and-`#[test]` pairs recoverable from `733af2a0^` pass:

```sh
# The 35 names come from the `#[cfg(target_os = "macos")]` + `#[test]` pairs
# in 733af2a0^, alternated into one nextest filter:
cargo nextest run --workspace --no-fail-fast -E 'test(/^(.*::)?(name1|name2|...)$/)'
# 35 tests run: 35 passed, 4532 skipped
```

33 in `eseqlisp` (`editor::tests` modal/dropdown/context-menu routing,
`ui::layout::tests` tree selection, `modal_layout_tests`, `tests`) and 2 in the
`metal_seq` bin (`state_values::tests`). The 36th was
`metal_seq_transport_view_buttons_switch_to_wide_arrangement_layout`, deleted in
`733af2a0` rather than ungated. Nothing platform-specific was buried.

The whole workspace on the same host: `cargo nextest run --workspace
--no-fail-fast` reports **4528 run, 4526 passed, 2 failed, 39 skipped** in 232s
debug. macOS compiles more targets than Linux does (Metal backend, capture
harness), which is why the run and skip counts are both higher than the Linux
4508/33 — the comparison to make is per test, not per total.

Neither failure is a Linux-port regression and both are filed:

| Test | Cause |
|---|---|
| `eseqlisp lang::sdf_codegen::tests::content_shader_corpus_emits_valid_wgsl` | `eseq-5ed`, recurring. Authored content shaders changed after `7396708d` and the two absolute digests were not re-captured. Reproduced at clean `HEAD` in a detached worktree with identical digests, so it is not working-tree drift, and it is not platform-specific. |
| `sequencer lisp_host::tests::vendored_dgen_abi_header_matches_staged_toolchain_header` | `eseq-linux.75`, macOS-only. `dgen_abi_v1.h` records one `Source-sha256` for a header that differs between the two pinned toolchain archives; `2f5a3cd9` bumped it to the Linux stage's header while the `arm64-apple-macos` pin stayed on the older one. Green on Linux, red on every Mac. |

The `eseq-linux.17` clip-math refactor was re-checked here too: every
clip/scissor/`gpu_geometry`/`metal_backend` test passes on macOS (118 run, 118
passed), so sharing `ui::gpu_geometry::{ScissorRect, ClipStack}` with the Metal
encoder did not regress the Metal-only side that Linux cannot exercise.

`eseq-linux.9`'s numeric-equivalence half closes here as well:
`cargo nextest run -p sequencer --lib -E 'test(/dgen_host_services_tests/)'`
passes all 10 on macOS. The `portable_*` tests pin the replacement FFT and the
`host_services_*` tests drive whatever backend the platform ships — vDSP here,
since `dgen_host_services.c` keeps the Accelerate calls under `#if
defined(__APPLE__)` and `build.rs` links the framework only on
`target_os = "macos"`. Both groups compare against the same naive f64 DFT, so
passing on both platforms is what turns "each matches the reference" into "the
portable FFT matches vDSP within tolerance".

## Which GPU backend the app actually got

`wgpu` does not fail when the backend the renderer expects is unavailable. It
falls back — to OpenGL when there is no usable Vulkan ICD, or to a software
rasterizer (llvmpipe/lavapipe) when there is no hardware adapter at all. Both
render correct pixels at completely different cost, so a fallback that is only
noticed later, as an unexplained performance number, is the failure mode this
guards against.

`ui/gpu_adapter.rs` therefore logs the selection at startup and asserts it:

```
eseq: selected vulkan adapter "Intel(R) UHD Graphics 620 (KBL GT2)" (IntegratedGpu, driver "Intel open-source Mesa driver" "Mesa 26.1.7")
```

By default the app refuses to start on the OpenGL fallback, on a `Cpu` adapter,
or on wgpu's dummy backend, and the refusal names the fix (install
`vulkan-intel`/`vulkan-radeon`/`nvidia-utils`, confirm with `vulkaninfo`).

| Variable | Effect |
|---|---|
| `ESEQ_GPU_BACKEND=vulkan\|metal\|dx12\|gl\|webgpu` | Require exactly this backend. Anything else is a hard startup failure. |
| `ESEQ_GPU_BACKEND=any` | Accept whatever was selected. |
| `ESEQ_ALLOW_GPU_FALLBACK=1` | Permit the OpenGL fallback and CPU rasterizers. Does not override an explicit `ESEQ_GPU_BACKEND` pin. |

CI sets `ESEQ_ALLOW_GPU_FALLBACK=1` because a GitHub runner has no GPU and
lavapipe is the correct thing for headless captures to run on there.

## Frame-time budget

### Generated SDF material cost

Measured by `eseqlisp_sdf_lighting_probe`; see
`docs/sdf-lighting-performance.md` for the method and the full table. On this
GPU, 96 generated SDF controls at 1920×1080:

| Generated lighting | Median | p95 |
|---|---:|---:|
| Full finite-difference lighting | 3.408 ms | 3.672 ms |
| Flat quality tier (`ESEQ_SDF_LIGHTING_QUALITY=flat`) | 2.714 ms | 3.021 ms |

This is the GPU half and it is not the constraint: the whole authored control
surface fits in a 16.6 ms frame with room to spare, which matches the observation
that SDF-heavy panels feel fine on this machine.

The macOS half of that comparison was captured on 2026-08-25 under
`eseq-linux.26`, on an Apple M1 Max through wgpu's Metal backend: 1.293 ms
median full against 1.281 ms flat, a 0.9% difference that a repeat run
reversed. The finite-difference lighting term is measurable on the UHD 620 and
below the noise floor on Apple silicon, which is the whole argument for keeping
the flat tier as an opt-in low-end fallback rather than a per-platform default.

### Application frame cost

The CPU half is measured in the running app. `ui/wgpu_frame_stats.rs` adds a
per-second aggregate to the wgpu shell, enabled with the same switch the Metal
backend uses:

```sh
ESEQLISP_PROFILE_UI=1 cargo run --release -p sequencer --bin metal_seq
```

It emits one line per second:

```
[ui-profile][wgpu] fps=… frames=… frame_avg=…ms frame_p95=…ms frame_max=…ms
  cpu_avg=…ms cpu_p95=…ms plan_avg=…ms scene_avg=…ms
  acquire_avg=…ms acquire_p95=…ms encode_avg=…ms
  prims/frame=… draws/frame=… buffers/frame=… buffer_kb/frame=…
  | scroll frames=… cpu_avg=…ms cpu_p95=…ms scene_avg=…ms acquire_avg=…ms
```

The three phases are deliberately separated, because "scrolling feels laggy" is
otherwise ambiguous between causes that need opposite fixes:

- `scene_avg` — rebuilding widget primitives: `collect_gpu_primitives`,
  offsetting, and segment splitting, summed over every visible tile. The wgpu
  shell has no retained scene cache (`ui/wgpu_app.rs` is the always-dynamic
  path), so this is paid in full on every redraw.
- `plan_avg` — the whole plan phase, `scene_avg` included. The remainder is
  text shaping plus one freshly created `wgpu::Buffer` per draw command;
  `buffers/frame` is that allocation load.
- `acquire_avg` — blocked in `get_current_texture`. Under `Fifo` with
  `desired_maximum_frame_latency: 2` this is swapchain backpressure, not work.
  It is the difference between a frame that is *expensive* and a frame that is
  *waiting its turn*, which is exactly the input-to-present versus frame-pacing
  distinction.

Tail values are reported alongside means because a gesture is judged by its
worst frames, and the scroll section is reported separately because a scroll
gesture makes every frame a full-cost redraw while an idle window redraws
almost nothing — averaging the two populations hides the interaction under test.

Budget, for the reference project on the reference machine: a scroll gesture
must hold `scroll cpu_p95` under one 60 Hz frame (16.6 ms). Treat a regression
against that as a bug, and attribute it with the phase breakdown above before
changing anything.

### Measured, 2026-08-25, scrolling a multi-track sequencer buffer

A representative steady-scroll second, with the three aggregates lined up:

```
[ui-profile][wgpu]      fps=8.1 cpu_avg=13.84ms cpu_p95=15.29ms plan_avg=10.97ms
                        scene_avg=5.57ms acquire_avg=0.06ms encode_avg=2.87ms
                        prims/frame=2907 draws/frame=889 buffers/frame=889
                        buffer_kb/frame=622.0 | scroll frames=9 cpu_p95=15.29ms
[ui-profile][sequencer] frames/s=8.0 frame_build_avg=50.08ms frame_build_max=110.77ms
                        render_avg=14.55ms gestures=475.15ms
[ui-profile][runtime]   relayout/s=16.9 relayout_avg=44.86ms reused=0 full=17
                        subtree=0 fail=- reactive/s=0.0 reruns=full:0 sub:0
```

Scrolling runs at **6–8 fps**, and the render path is not why:

| | per frame | per second |
|---|---:|---:|
| Full relayout (~2.1 per frame) | ~94 ms | **~758 ms** |
| wgpu render total | 13.8 ms | 116 ms |
| — of which widget scene rebuild | 5.6 ms | 47 ms |
| — of which plan minus scene (889 buffers, 622 KB) | 5.4 ms | 45 ms |
| — of which encode + submit | 2.9 ms | 24 ms |
| — of which swapchain wait | 0.06 ms | 0.5 ms |

Three things this settles:

- **Layout is the cost, not rendering.** ~76% of wall-clock during a scroll is
  spent in full relayouts. The entire wgpu render path is ~12%.
- **Swapchain backpressure is not involved at all.** `acquire_avg` is 0.06 ms.
  Frame pacing and input-to-present latency are ruled out; this is pure CPU.
- **Nothing is being reused.** `reused=0 full=17 subtree=0`, with `reactive/s=0`
  and zero buffer reruns — the widget *tree* is not being rebuilt, yet the
  layout is recomputed from scratch about twice per frame. `fail=-` means no
  reuse-failure reason was recorded either, which is itself the clue: several
  callers of `relayout_current_tree` set `self.current_layout = None`
  immediately beforehand (`runtime.rs` — `invalidate_layout`,
  `flush_deferred_layout_invalidation`, `set_layout_cell_dimensions`,
  `set_text_measurer`, `set_widget_id_offset`, `set_layout_aspect`). That
  discards the very `previous_layout` the reuse path needs, so those call sites
  take the full path *by construction* and report no reason for it.

Attributing which of those callers fires during a scroll is the first task in
`eseq-pzp`. Note that `set_layout_content_scroll` already documents this exact
hazard and deliberately does not invalidate.

### Attributed and fixed, 2026-08-25 (`eseq-pzp`)

`relayout_current_tree` is now `relayout_current_tree_because(cause)`, taking a
`&'static str` from every caller, and it falls back to `cleared:<cause>` when
the caller nulled `current_layout` and there is therefore no reuse-failure
reason to report. `[ui-profile][runtime] fail=` names the setter instead of
printing `-`, and `[ui-trace] fail=` does the same per cycle.

With that in place, the ~2.1 relayouts per scroll frame resolve.

#### The dominant cost: two derivations of the tile layout viewport disagreed

`border_width_px` is authored in the 2x design-pixel space. Everything that puts
it on screen scales it — both backends draw the tile border at
`ui_design_px(border_width_px)`, and `tile_content_border_insets` maps pointer
coordinates through the same scale. Input routing inset the tile's *layout*
viewport the same way (`metal_tile_content_viewport`, `editor/mod.rs`), but the
frame builder used the **raw** value (`metal_tile_inner_extents`, `ui/frame.rs`).

`ui_px_scale()` is `window_scale_factor / 2.0`, and only the wgpu shell ever
calls `set_ui_scale_factor` — the Metal path leaves it at the 2.0 default. So on
macOS the scale is exactly 1.0, the two derivations agree, and nothing happens.
On this machine at window scale 1.0 the scale is 0.5 and they diverge:

```
scale=2.0  ui_px_scale=1.0  routing=(39.6, 9.8)  frame=(39.6, 9.8)  relayouts: 0 + 0
scale=1.0  ui_px_scale=0.5  routing=(39.8, 9.9)  frame=(39.6, 9.8)  relayouts: 1 + 1
```

Every routed input event set one viewport and re-laid the whole tree out for it;
the next frame set the other back and re-laid it out again. Two full
`LayoutEngine` passes per frame, on **any** buffer, with no scroll widget
involved — which is why it bites the *sequencer* and *fx* buffers, neither of
which contains one. Scroll is simply the gesture that delivers a dense stream of
routed events, so it is where the cost becomes visible. It also matches every
number in the capture: `full=17` with `reused=0`, `fail=-` (both call sites null
`current_layout` first), `frame_build_avg=50.08ms` for the frame-side pass, and
`gestures=475.15ms` for the routed-event side.

The frame builder now scales the inset the same way everyone else does. That
also fixes a latent correctness bug: at any non-reference scale the widget tree
was being laid out against a viewport that did not match the border actually
drawn around it.

Regression test: `routed_input_does_not_relayout_at_a_non_reference_window_scale`.

#### Two secondary scroll-path relayouts

Both are real and both are fixed, but neither is what bites the reported
buffers — the first applies to the *mixer* buffer, which is where `scroll`
widgets are actually used.

1. **Every scroll gesture over a `scroll` container invalidated the layout.**
   `handle_touchpad_scroll_impl` invalidated on `widget_type == "scroll"`
   unconditionally. A `scroll` container lays its child out at full content
   height and applies the offset at render time, so the offset changes no
   geometry the layout engine computed. The one exception is a virtualized
   stack inside it — `virtual-v-stack` is the only reader of
   `LayoutCtx::scroll_offset_y`, and it materializes only the visible window.
   The invalidation is now gated on the scroll subtree actually containing one
   (`scroll_layout_depends_on_offset`, stopping at nested `scroll` boundaries,
   which install their own offset). A repaint is still scheduled — the dirty
   scroll-key path in `widget_render::scroll` already carries offset-only
   changes through `dirty_widget_ids`, deliberately without bumping the global
   widget state generation.
2. **Tile routing settled the deferred invalidation once per raw event.**
   `Editor::route_event_to_tile` called `set_layout_viewport_exact` before
   dispatching each event, and that setter's unchanged-viewport branch flushes
   `deferred_layout_invalidated`. So whenever anything had queued a deferred
   relayout, the *next* raw event performed it — turning the burst coalescing
   `invalidate_layout_deferred` exists to provide back into one full
   `LayoutEngine` pass per event. Nothing on the reported buffers' scroll path
   sets that flag, so this did not contribute to the captured numbers, but it
   does defeat coalescing wherever the flag is set (virtualized lists, tree
   expand/collapse) during any input burst. Routing now uses
   `set_layout_viewport_exact_deferring`, which leaves the pending invalidation
   for the frame builder (`build_tiled_render_frame_impl` already flushes it
   before snapshotting tile revisions). Hit tests during the burst keep using
   the previous layout, which is what `invalidate_layout_deferred` documents.

Regression coverage in `crates/eseqlisp/src/editor/tests.rs`, all three failing
before the change: `routed_input_does_not_relayout_at_a_non_reference_window_scale`,
`touchpad_scroll_over_plain_scroll_container_does_not_relayout`, and
`tiled_touchpad_scroll_burst_relayouts_virtual_stack_once_per_frame`.

The render-side items below are untouched and remain worth doing — the
measurement puts them at ~12% of the scroll bill, not the dominant cost.

Measured values are recorded in the bead `eseq-linux.12` notes as they are
captured. Two cases must be in any capture, because both are reported as slow on
this machine: scrolling a multi-track sequencer buffer vertically, and scrolling
a long fx chain horizontally. SDF-heavy panels are reported as fast, which the
GPU numbers above agree with.

Scroll is where the wgpu shell is structurally worse off than the Metal one, and
`eseq-pzp` tracks fixing it:

- The shell has no retained scene cache and no compiled-widget-run cache, so
  every visible tile rebuilds every primitive on every frame. `MetalBackend`
  routes the same work through `widget_scene_for_layout`, which is already free
  of any Metal type and is portable as-is.
- It creates one `wgpu::Buffer` per draw command per frame and drops them all at
  end of frame, so allocation load scales with draw count rather than with what
  changed.
- A pure scroll frame misses the scene cache on *both* backends, because
  `scroll_top` is part of the cache key and the retained-run path only runs when
  a widget is dirty or animating. A fast CPU absorbs that; a Kaby Lake-R laptop
  does not.

Capture the profile before changing any of it — the three phases exist so the
fix targets whichever one the numbers name.

## CI

`.github/workflows/linux.yml` runs `cargo check --workspace --all-targets` and
the test command above on `ubuntu-24.04`, so the port cannot rot silently.

Four of its system dependencies are load-bearing and easy to get wrong:

- `libasound2-dev` — cpal's ALSA backend; the workspace does not build without it.
- `libpipewire-0.3-dev` — native graph-clock discovery for PipeWire-routed ALSA
  output; without its headers and pkg-config metadata the Linux sequencer does
  not build.
- `mesa-vulkan-drivers` — lavapipe, so the headless wgpu captures find an
  adapter instead of printing `SKIPPED: no wgpu adapter available` and asserting
  nothing.
- `fonts-jetbrains-mono` — glyph metrics come from `fontdb`'s *system* font
  database, and several layout tests measure against that exact family. Without
  the font those tests fail rather than skip.
