# tinyseq

A terminal-based step sequencer for sample playback, built in Rust with a lock-free audio engine. Inspired by hardware sequencers like the Cirklon and Elektron boxes.

![tinyseq runs in your terminal](https://img.shields.io/badge/interface-TUI-blue)
![built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)

## Features

- **Step sequencer** with up to 64 tracks and 64 steps per track
- **Per-step parameter locks** (p-locks) for duration, velocity, transpose, chop, and two aux sends
- **Multi-pattern bank** with clone, delete, and instant switching
- **Per-track effects chain**: manually added built-in and custom DSP effects written in a Lisp dialect (DGenLisp) that hot-compile into the audio graph
- **Embedded control Lisp via `eseqlisp`** for scratch scripting, hook-based pattern automation, and in-app instrument/effect editing
- **Global reverb bus** with per-track send levels
- **Polyphonic voice pool** with chord recording
- **Keyboard playing and recording** with quantized step input
- **Sample browser** with folder tree navigation and audition
- **Per-track step count** (polymetric patterns) with page navigation
- **Lock-free audio engine** (C-based audiograph library) with real-time safe graph editing
- **Piano keyboard visualizer** showing currently sounding notes
- **Mouse support** for clicking steps, tracks, params, and pattern buttons
- **ratatui TUI** that fits in any terminal

## Requirements

- **macOS only** -- the DGenLisp effect compiler depends on Metal for GPU-accelerated DSP
- Rust toolchain
- C compiler (Xcode command line tools)

## Building

```
cargo build --release
```

The audiograph C library is compiled automatically via `build.rs` + `cc`.

## Running

```
cargo run --release
```

On first launch, tinyseq creates `samples/` and `effects/` directories in the working directory. Drop `.wav` files into `samples/` (nested folders are fine) and they'll appear in the sidebar browser.

## Headless UI capture

The Metal sequencer can render a named UI buffer from a deterministic,
Lisp-defined project fixture without opening the interactive app or an audio
device. This is useful for visually iterating on instrument, process, effect,
and sequencer panels. See [Metal sequencer UI capture](../../docs/metal-seq-ui-capture.md).

## Quick start

1. Press **Ctrl+N** to open the sample browser
2. Navigate folders with **Up/Down**, expand/collapse with **Enter**
3. Select a `.wav` file to add it as a new track
4. Press **Tab** to focus the step grid
5. Press **Enter** to toggle steps on/off
6. Press **Space** to play/stop

## Controls

### Global

| Key | Action |
|-----|--------|
| `q` | Quit |
| `Space` | Play / Stop |
| `Tab` / `Shift+Tab` | Cycle focus: Grid -> Params -> Grid |
| `Ctrl+N` | Open sample browser (add track) |
| `Ctrl+L` | Open effect picker / edit custom effect |
| `Ctrl+A` | Select all active steps |
| `Esc` | Clear selection |
| `/` | Focus sidebar search, disarm all tracks |

### Step grid (Cirklon region)

| Key | Action |
|-----|--------|
| `Left` / `Right` | Move cursor between steps |
| `Up` / `Down` | Move cursor between tracks |
| `Alt+Left` / `Alt+Right` | Jump 4 steps |
| `Shift+Left` / `Shift+Right` | Extend selection |
| `Shift+Up` / `Shift+Down` | Adjust selected step's parameter value |
| `Enter` | Toggle step on/off |
| `Backspace` / `Delete` | Clear selected steps |
| `d` `v` `a` `b` `t` `c` | Switch active parameter (duration, velocity, auxA, auxB, transpose, chop) |
| `i` | Enter **Insert mode** (type values directly) |
| `s` | Enter **Select mode** (multi-select steps) |
| `r` | Enter **Arm mode** (keyboard recording) |
| `x` `x` (double tap) | Clear entire pattern |

### Param region (bottom panels)

**Track params** (left panel):

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate: Gate, Attack, Release, Swing, Steps, Send, Poly |
| `Left` / `Right` | Adjust value |
| `Enter` | Toggle boolean params (Gate, Poly) |

**Effects** (right panel):

| Key | Action |
|-----|--------|
| `Left` / `Right` | Switch between loaded effect slots and Reverb |
| `Up` / `Down` | Navigate effect parameters |
| `Shift+Up` / `Shift+Down` | Adjust parameter value |
| `Enter` | Toggle on/off or cycle enum values |

### Keyboard playing (Arm mode)

| Key | Action |
|-----|--------|
| `r` | Enter arm mode, then click track arm dots to arm/disarm |
| Piano row (`a`-`'`) | Play notes (chromatic keyboard layout) |
| `z` / `x` | Octave down / up |
| `,` | Toggle recording (writes notes into pattern while playing) |
| `[` / `]` | Adjust record quantize threshold |

### Patterns

| Key | Action |
|-----|--------|
| Click pattern numbers | Switch patterns |
| Click `[+]` | Clone current pattern |
| Click `[x]` | Delete current pattern |

### Sample browser (sidebar)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate tree |
| `Enter` | Expand folder / select sample |
| Type characters | Filter by name |
| `Backspace` | Clear filter character |
| `Ctrl+A` | Switch to audition mode (preview samples on current track) |

## Architecture

```
src/
  main.rs          -- startup, terminal setup, audio graph wiring
  sequencer.rs     -- shared state (lock-free atomics), clock, pattern bank
  audio.rs         -- cpal output stream, per-block processing
  sampler.rs       -- sample loading, playback with envelope
  voice.rs         -- polyphonic voice allocation pool
  filter.rs        -- biquad filter DSP
  delay.rs         -- delay line DSP
  reverb.rs        -- Dattorro plate reverb
  effects.rs       -- unified effect slot system with p-locks
  lisp_host.rs   -- DGenLisp custom effect compilation and loading
  audiograph.rs    -- FFI bindings to the C audiograph engine
  ui/
    mod.rs         -- App struct, layout, regions
    draw.rs        -- ratatui rendering
    input.rs       -- keyboard/mouse dispatch
    cirklon.rs     -- step grid interaction
    params.rs      -- track param and effect param editing
    tracks.rs      -- track management, graph wiring
    effects.rs     -- effect chain UI logic
    browser.rs     -- sample folder tree browser
```

The audio runs through a C-based lock-free graph engine (`audiograph/`) that supports real-time node addition, removal, and parameter changes without blocking the audio thread.

## Custom effects

tinyseq supports custom DSP effects written in DGenLisp, a Lisp dialect that compiles to native shared libraries. Place `.dgenlisp` source files in the `effects/` directory and load them via **Ctrl+L**. Effects are hot-compiled in a background thread and patched into the audio graph on completion.

## Instrument probe

`instrument_probe` is a command-line render harness for DGenLisp instruments. It loads an instrument through the same host-side compile/load/init path used by tinyseq, sends a note into the instrument, renders audio blocks, and reports basic signal statistics. Use it when adding or changing instruments so quiet output, silent output, broken parameter defaults, and missing local data files are caught before testing in the DAW/TUI.

Run it against a saved instrument name:

```sh
cargo run --bin instrument_probe -- emulations/monomachine-dpro-wave-v2 \
  --frames 4096 \
  --min-peak 0.01 \
  --min-rms 0.001
```

Or run it against a direct source path:

```sh
cargo run --bin instrument_probe -- instruments/emulations/monomachine-dpro-wave-v2/dsp.lisp \
  --midi-note 60 \
  --velocity 0.8
```

Useful options:

| Option | Meaning |
|--------|---------|
| `--frames N` | Total frames to render, default `44100` |
| `--block-size N` | DSP block size, default `128` |
| `--sample-rate N` | Render sample rate, default `44100` |
| `--midi-note N` | MIDI note sent to the instrument, default `69` |
| `--velocity V` | Note velocity from `0..1`, default `1` |
| `--gate-frames N` | Number of frames the gate stays high, default is the full render |
| `--param name=value` | Override an instrument parameter; repeat the flag for multiple params |
| `--min-peak V` | Exit with failure if the rendered peak is below `V` |
| `--min-rms V` | Exit with failure if the rendered RMS is below `V` |
| `--json` | Print a machine-readable report |

For direct paths, relative assets such as wavetable JSON files are resolved from the source file's parent directory. For saved instrument names, assets are resolved from the saved instrument directory, matching the normal host loader.

## `eseqlisp` integration

tinyseq now embeds [`eseqlisp`](https://github.com/universalsequences/eseqlisp) for control scripting and editing.

- Custom instrument/effect editing runs inside the terminal UI instead of shelling out to `vim`
- `Ctrl+G` opens a fullscreen scratch buffer for live sequencer scripting
- Scratch can query and mutate pattern data, register timed hooks, and is saved with project files
- Project load restores scratch text and cursor position, but does not auto-run saved scratch code or hooks

`eseqlisp` is intentionally separate from DGenLisp:

- **DGenLisp** is the DSP/instrument/effect language
- **eseqlisp** is the control/UI/agent language

## Dependencies

- **ratatui** + **crossterm** -- terminal UI
- **cpal** -- cross-platform audio output
- **hound** -- WAV file loading
- **cc** -- C compiler integration for audiograph

## License

The audiograph engine is licensed under its own terms (see `audiograph/LICENSE`). The rest of the project is unlicensed / public domain -- do whatever you want with it.
