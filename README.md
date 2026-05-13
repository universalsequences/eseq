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
<img width="1321" height="889" alt="Screenshot 2026-05-12 at 11 37 57 PM" src="https://github.com/user-attachments/assets/43757230-fff5-4e2a-9bf9-b4ee07a04915" />
