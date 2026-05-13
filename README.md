# eseq

Rust workspace for eseqlisp, musicplayer, and sequencer.

## Layout

- `crates/eseqlisp` - Lisp UI/runtime crate
- `crates/musicplayer` - music player app
- `crates/sequencer` - sequencer app and audio engine

## Commands

```sh
cargo check --workspace
cargo run -p musicplayer
cargo run -p sequencer --bin sequencer
cargo run -p sequencer --bin metal_seq
```
