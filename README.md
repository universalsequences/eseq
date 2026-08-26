# eseq

A live sequencer and audio engine for macOS and Linux, with a Lisp-based
UI/runtime and DSP that is compiled to native code on the fly. macOS renders
through Metal with CoreAudio output; Linux renders through wgpu (Vulkan) with
cpal/ALSA output.

<img width="1321" height="889" alt="Screenshot 2026-05-12 at 11 37 57 PM" src="https://github.com/user-attachments/assets/43757230-fff5-4e2a-9bf9-b4ee07a04915" />

## System requirements

On every platform you need the **Rust toolchain** — install via
[rustup](https://rustup.rs) — and `curl` for the two fetch scripts described
under [Fetching the DGen compiler](#fetching-the-dgen-compiler).

### macOS

- **Xcode Command Line Tools** — install with `xcode-select --install`
  (provides the system linker used when building the Rust workspace)

### Linux

- **x86_64 Linux with a native Wayland session.** The app runs winit's Wayland
  backend; HiDPI/fractional scaling (e.g. a compositor monitor scale of 1.6)
  is handled via `WindowEvent::ScaleFactorChanged` and
  `wp-fractional-scale-v1`. Running under X11/XWayland is unsupported and
  untested, and will not pick up the compositor scale.
- **Audio development libraries** — `libasound2-dev` and
  `libpipewire-0.3-dev` on Debian/Ubuntu; `alsa-lib` and `pipewire` on Arch.
  CPAL uses ALSA for output, while the PipeWire API supplies the active graph
  clock when `pcm.default` routes through the PipeWire ALSA plugin.
- **Windowing libraries** — `libwayland-dev`, `libxkbcommon-dev`, and the X11
  headers (`libx11-dev`, `libxcursor-dev`, `libxi-dev`, `libxrandr-dev`) that
  winit links against; on Arch these come with `wayland` and `libxkbcommon`.
- **A Vulkan driver for your GPU** — e.g. mesa's `vulkan-intel` or
  `vulkan-radeon`. The renderer runs on wgpu, and startup logs the selected
  adapter/backend and *asserts* it got a real Vulkan GPU: a silent fallback to
  OpenGL or a software rasterizer (llvmpipe/lavapipe) is rejected with a hard
  error rather than rendering slowly. `ESEQ_ALLOW_GPU_FALLBACK=1` downgrades
  that rejection to a warning, and `ESEQ_GPU_BACKEND=<vulkan|gl|any>` pins or
  relaxes the backend requirement.
- **JetBrains Mono** installed as a system font (`fonts-jetbrains-mono` /
  `ttf-jetbrains-mono`) — UI glyph metrics are resolved through the system
  font database.

See `.github/workflows/linux.yml` for the exact package list the Linux CI job
installs.

## Fetching the DGen compiler

Instrument and effect DSP is compiled at runtime by the `DGenLisp` tool, which
shells out to a hermetic clang/lld toolchain — never to a system compiler.
Neither binary is tracked in git; both are pinned per target in lock files
(`content/dgenlisp.lock`, `content/dgen-toolchain.lock`) and installed by two
idempotent, sha256-verified scripts. Run once per fresh checkout:

```sh
./scripts/fetch_dgenlisp.sh        # the DGenLisp compiler
./scripts/fetch_dgen_toolchain.sh  # the clang/lld stage it invokes
```

Both install under `crates/sequencer/tools/` (gitignored). Anything that needs
the compiler and cannot find it hard-fails with a message naming the fetch
command. `ESEQ_DGENLISP_TOOL=/abs/path` overrides the pinned compiler with a
locally built one. Maintainers publishing a new compiler should follow the
complete [Linux DGenLisp release recipe](docs/dgenlisp-release.md), including
its container, binary-verification, draft-recovery, and lock re-pin steps.

## Running

Build and run the release sequencer (same command on macOS and Linux; the
Linux build automatically pulls in eseqlisp's wgpu backend):

```sh
cargo run -p sequencer --release --bin metal_seq
```

The first build compiles the full workspace and takes a while; subsequent runs
are incremental.

## Realtime audio on Linux

Audio runs, but expect **glitches under load** until the process can schedule
its audio threads with realtime priority. At startup eseq walks a fallback
ladder and reports the scheduling policy each audio thread *actually achieved*:

1. **Direct `SCHED_FIFO`** (full priority) — needs a nonzero `RLIMIT_RTPRIO`,
   which an unconfigured host does not grant. With the grant, graph workers
   run at FIFO priority 20 and the cpal callback thread at 21.
2. **RealtimeKit fallback** (zero configuration) — when direct promotion is
   denied, a helper asks rtkit-daemon (present on any PipeWire desktop) for
   `SCHED_RR` at rtkit's capped priority. This is the out-of-the-box realtime
   path; the callback thread keeps priority ≥ the workers within the cap.
3. **`SCHED_OTHER`** — if rtkit is also unavailable, audio continues at normal
   priority after a one-shot warning (`direct SCHED_FIFO promotion was denied
   and the RealtimeKit fallback was unavailable ...`). Nothing is broken, but
   audio dropouts under CPU load are expected.

For the full-priority path on a dev box, install the shipped PAM limits
drop-in, join the `realtime` group, and log out/in:

```sh
sudo groupadd --system --force realtime
sudo install -Dm644 crates/sequencer/audiograph/packaging/eseq-realtime.conf \
  /etc/security/limits.d/95-eseq-realtime.conf
sudo usermod -aG realtime "$USER"
```

Log out of the session completely and log back in (a new terminal in the old
session is not enough), then verify with `ulimit -r` (expect 95) before
launching. A systemd service instead needs `LimitRTPRIO=95` and
`LimitMEMLOCK=infinity` directives. Details, verification, and the rtkit
`RLIMIT_RTTIME` contract are documented in
[crates/sequencer/audiograph/README.md](crates/sequencer/audiograph/README.md).

To verify what you got: set `TINYSEQ_AUDIOGRAPH_RT_LOG=1` for per-thread
promotion logs (workers at `SCHED_FIFO` base priority, callback at base+1 —
or `SCHED_RR` at rtkit's cap), or inspect a running thread from another shell
with `chrt -p <tid>`.

### Audio environment knobs

- `TINYSEQ_AUDIOGRAPH_RT` — enable/disable realtime scheduling (default on
  when workers > 0)
- `TINYSEQ_AUDIOGRAPH_RT_PRIORITY` — base priority for workers (default 20;
  the callback runs at base+1)
- `TINYSEQ_AUDIOGRAPH_RT_LOG` — per-thread scheduling promotion logs
- `TINYSEQ_AUDIOGRAPH_WORKERS` — worker thread count; note `0` also disables
  realtime scheduling unless `TINYSEQ_AUDIOGRAPH_RT=1` is set explicitly
- `TINYSEQ_AUDIO_TRACE` — audio stream tracing

cpal talks plain ALSA. On a PipeWire desktop that routes through
`pipewire-alsa`, so the "default" ALSA device is PipeWire's graph: device
selection and sample routing follow your desktop audio setup, with PipeWire's
buffering on top of eseq's own.

## Linux development notes

- **Tests**: `cargo nextest run` is the expected runner
  (`cargo install cargo-nextest --locked`). The 16 MiB `RUST_MIN_STACK` test
  stack budget is applied automatically by `.cargo/config.toml` — do not add
  local overrides.
- Some tests are `cfg(target_os = "macos")`-gated, so Linux pass/skip counts
  differ from the macOS baseline in
  [docs/test-suite-performance.md](docs/test-suite-performance.md). The Linux
  workspace baseline and every skip's reason are recorded in
  [docs/linux-validation.md](docs/linux-validation.md).
- Keep Cargo target directories off `/tmp` on tmpfs-rooted machines — a small
  tmpfs cannot hold one.
- **GPU expectations**: the renderer is tuned for Apple GPUs (see
  [UI_PERFORMANCE_TUNING.md](UI_PERFORMANCE_TUNING.md)); an integrated GPU is
  a different performance regime. The Linux frame-time budget measured on the
  Intel UHD 620 reference machine lives in
  [docs/linux-validation.md](docs/linux-validation.md). The startup log shows
  which adapter and wgpu backend were selected.

## Layout

- `crates/eseqlisp` — Lisp UI/runtime crate
- `crates/sequencer` — sequencer app and audio engine
  - `crates/sequencer/audiograph/` — vendored copy of
    [audiograph](https://github.com/universalsequences/audiograph), the C
    live-editable audio graph engine, copied verbatim from its own repo
  - `crates/sequencer/tools/DGenLisp-<target>` — prebuilt, target-specific
    binaries of
    [dgen-audio](https://github.com/universalsequences/dgen-audio)'s DGenLisp,
    a Lisp-to-dylib DSP compiler; the sequencer shells out to it to compile
    instrument/effect patches to native shared libraries at runtime

```sh
cargo check --workspace
```

## License

This project is licensed under the GNU General Public License v3.0 — see
[LICENSE](LICENSE).

The exception is the vendored [audiograph](https://github.com/universalsequences/audiograph)
sources under `crates/sequencer/audiograph/`, which are copied verbatim from
their upstream repo and remain MIT licensed — see
[crates/sequencer/audiograph/COPYING](crates/sequencer/audiograph/COPYING).
