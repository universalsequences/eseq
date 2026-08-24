# eseq

A live sequencer and audio engine for macOS, with a Lisp-based UI/runtime and
DSP that is compiled to native code on the fly.

<img width="1321" height="889" alt="Screenshot 2026-05-12 at 11 37 57 PM" src="https://github.com/user-attachments/assets/43757230-fff5-4e2a-9bf9-b4ee07a04915" />

## System requirements

- **macOS** — Apple platforms only (Metal renderer, CoreAudio output)
- **Rust toolchain** — install via [rustup](https://rustup.rs)
- **Xcode Command Line Tools** — install with `xcode-select --install`

The command line tools are needed for more than building the Rust workspace:
instrument and effect DSP is compiled at runtime by the bundled `DGenLisp`
tool, which invokes `clang` at its standard location (`/usr/bin/clang`). A
packaged/self-contained compiler is not implemented yet, so a working clang
install on the machine is expected.

## Running

Build and run the release sequencer:

```sh
cargo run -p sequencer --release --bin metal_seq
```

The first build compiles the full workspace and takes a while; subsequent runs
are incremental.

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
