# Text backend golden captures

Each subdirectory is one capture from the `eseqlisp_text_capture` harness
(`crates/eseqlisp/src/text_capture.rs`, schema v1), named
`<rasterizer>-<os>-<arch>`. Every capture holds:

* `metrics.json` — cell/line metrics and per-character advances for the fixed
  font-size × scale-factor matrix.
* `text.png` — the fixed buffer in `../buffer.txt`, composited by the harness'
  own CPU rasterizer so no graphics API is involved.

These are reference data, not assertions. Judging the deltas between them is
`eseq-linux.20`; nothing here decides whether a delta is acceptable.

## Captures

### `fontdue-macos-arm64`

* Source commit: `7664460f` (branch `eseq-linux`), harness run unmodified.
* Host: MacBook Pro, Apple M1 Max, macOS 26.5.1 (25F80), arm64.
* Display: built-in Liquid Retina XDR, 3024×1964, backing scale factor 2.0.
* Resolved fonts: `JetBrainsMono-Regular` (mono), `Helvetica` (proportional).

### `coretext-macos-arm64`

* Source commit: `10c0e912` — i.e. `57b76e2a^`, the last commit before
  "feat(eseqlisp): replace CoreText glyph atlas" deleted the CoreText atlas and
  dropped `objc2-core-text`. This is the only way to produce CoreText numbers.
* Host and display: same machine and settings as above.
* Resolved fonts: `JetBrainsMono-Regular` (mono), `.SFNS-Regular` (proportional).

Reproducing it: check `57b76e2a^` out in a scratch worktree, copy
`src/text_capture.rs`, `src/bin/eseqlisp_text_capture.rs` and
`tests/fixtures/text-capture/buffer.txt` in, add `pub mod text_capture;` to
`crates/eseqlisp/src/lib.rs`, and replace `src/text_capture/adapter.rs` with
`coretext-macos-arm64/adapter-shim.rs.txt` (kept here verbatim for exactly this
reason). No `Cargo.toml` change is needed — `image` and `serde_json` are already
dependencies at that commit. Then run the binary with
`--name coretext-macos-arm64`.

The shim drives the production CoreText atlas rather than reimplementing
rasterization: metrics come from the same `CTFont` calls the atlas makes, and
glyph coverage is read back out of the atlas' `MTLTexture`, which at that commit
is the only copy of the rasterized pixels.

## Determinism

Two consecutive runs on the same host produced byte-identical `metrics.json` and
`text.png` for both captures.

## Not captured here

`fontdue-linux-*` is produced on the Linux host and lands beside these.
