# Patcher probes: live views of DSP signals

Status: spec rev 1, 2026-10-03. SHIPPED 2026-10-03: compiler in dgenlisp-v0.1.32 (dgen-audio PR #22, macOS pinned; Linux still v0.1.20, eseq-2vlh). Implementation deviations: probe channels are listed in both `outputs[]` and `probes[]`; effect probe tokens live in `state[0]` (generation-tagged, no free header slot); scope y-range easing is publisher-side (`ProbeFrame.display_range`). Epic bead: `eseq-d1xr` (children .1-.7 map to §9 slices; see `bd show eseq-d1xr`).

## 1. Problem

Patching DSP blind. In a patch like `freq-to-delay → / 512 → allpass-diffuser`
or `… → clip 50 5000 → biquad`, nothing shows what value flows down a cable,
so authors guard with defensive `clip`s and guess ranges. Max/Gen solve this
with `number~` and `scope~`: objects that show a signal live, inside the patch.

Two things make this non-trivial in DGen:

1. **Getting values out.** The signal lives inside compiled C. Reading node
   state through audiograph's watchlist (`add_node_to_watchlist` /
   `get_node_state_into`) copies the whole state buffer. That includes every
   delay line, and all we want is a few floats.
2. **Reachability.** A view is a graph endpoint that never reaches an `out`.
   Unless something keeps it as a root, the compiler eliminates it as dead code.

## 2. Decision

A **compiler-native `probe` primitive whose output becomes a hidden output
channel.**

```lisp
(probe x)                         ; passthrough: returns x
(probe x @id "cutoff" @view scope)
```

- `probe` is an identity on `x`, so it composes inline:
  `(biquad in (probe (clip f 50 5000) @id "cut") 0.9 1 1)`. Left dangling as
  a top-level form, it still compiles: **every probe is a graph root**, just
  as every `out` is.
- The compiler places each probe on an **output channel it assigns itself**,
  after every user-declared channel (audio, `@modulator`, `@amp`). Authors
  never choose a channel number, so a probe can't collide with an existing
  output, and adding one doesn't renumber them.
- The manifest lists probes in a new `probes[]` array. Hosts treat probe
  channels as non-audio, as they already do for `@amp`.

### 2.1 Why hidden outputs and not a memory region

The alternative was to poke into a tensor and read it from state memory. We
rejected it:

- DGen effect state is double-buffered (read and write regions; see
  `dgen_ffi.rs` and the effect memory-layout notes). A reader would have to
  know which buffer is current. Instrument state is per voice as well.
- Output buffers come with no ambiguity: our Rust wrappers already hold
  them when `process` returns (`record_dgen_voice_amp` in
  `instrument_storage.rs`, `dgenlisp_wrapper_process` in `dgen_ffi.rs`).
- `@amp` already proves the path end to end: compiler attribute → manifest
  channel → buffer sizing by highest channel (`c37ee81aa`) → routing
  exclusion (`app/graph/mod.rs` `manifest_audio_output_channels`) → read in
  the audio thread. Probes generalize that path from one flag to N channels.

The cost is one buffer write per probe per sample, the same as a `poke`.

### 2.2 Why native and not `(out x K @probe)`

- It's a passthrough, so it can probe mid-expression without restructuring.
- The compiler assigns channels, so there's no K for authors or the patcher to
  manage.
- The manifest describes probes directly instead of hiding them among the outputs.
- Probes stay alive because the compiler says so, not because the code emitter
  happens to produce something that survives dead-code elimination.

## 3. Compiler (dgen, `~/code/swift/dgen`)

### 3.1 Surface

`(probe <signal> [@id <string>] [@view number|scope|meter] [@name <string>])`

- Returns `<signal>` unchanged; no extra DSP on the audio path.
- `@id`: stable identity for the host and the patcher. If omitted, the compiler
  assigns `probe-<ordinal>` in evaluation order. Hand-written sources may omit
  it; the patcher always writes one (§6.2).
- `@view`: an opaque hint passed through to the manifest; the compiler doesn't
  interpret it. Default `number`.
- v1 accepts **scalar signals only**. A tensor, `signalTensor` or batched
  signal is an error, as it already is for `@amp`
  (`LispEvaluator.swift` `evalOutput`). Tensor probes are a follow-up (§8).
- Each evaluation of a `probe` form makes a separate probe. A probe inside a
  `defmacro` body that expands three times makes three. Repeated `@id`s are
  allowed; the manifest records an `occurrence` index (0, 1, 2…) in evaluation
  order.

### 3.2 Lowering

Record a `ProbeInfo { signal, id, occurrence, view, name }`, analogous to
`OutputInfo`. After the whole source has been evaluated, assign channels
`maxUserChannel + 1 + i` in probe order and emit them as ordinary scalar
outputs. Doing this after evaluation avoids collisions with an `out` that
appears later in the source.

### 3.3 Manifest

```json
"probes": [
  { "id": "cut", "occurrence": 0, "channel": 3, "view": "scope", "name": null }
]
```

The `outputs[]` entries for these channels may be present or absent; hosts key
off `probes[]`. Document it in `Sources/DGenLisp/README.md` next to `@amp`.

### 3.4 Strip flag

Add `--no-probes`, which lowers `probe` to plain identity and emits no
channels. Hosts don't need it for correctness, because probe channels are never
routed. It's for offline tools that compare renders across source revisions.

Ship it as a DGenLisp release and bump `content/dgenlisp.lock` for both
targets. Until the Linux target is bumped, sources that use `probe` won't
compile there, so factory content must not use `probe` before then.

## 4. Host: manifest + routing (eseq)

- `dgen_manifest.rs`: parse `probes[]` into `DGenManifest.probes:
  Vec<DGenProbe { id, occurrence, channel, view }>`. `n_outputs` already
  sizes by the highest declared channel, so include probe channels in that max
  (they may be absent from `outputs[]`).
- Replace the amp-only exclusions with one helper,
  `manifest_non_audio_channels()` = mod outputs ∪ `@amp` ∪ probes, used by:
  - `app/graph/mod.rs` `manifest_audio_output_channels`
  - effect chain graph routing (`lisp_host/dgen/effect_chain_graph.rs`)
  - every other site that currently filters `amp_output_channel`
- Offline tools: `tools/audition` and the PM verify drivers already drop the
  amp flag from rendered audio. They must drop probe channels the same way,
  driven by the manifest.
- Test: a manifest with audio on 1, `@amp` on 2 and probes on 3 and 4 routes
  only channel 1 and sizes 4 buffers. This extends
  `amp_output_channel_is_never_routed_as_audio`.

## 5. Host: capture on the audio thread

### 5.1 Where

- **Instruments:** at the point where `record_dgen_voice_amp` runs, after a
  voice block renders, call `record_dgen_voice_probes(slot_id, out, nframes)`.
- **Effects:** in `dgenlisp_wrapper_process`, after `process_fn` returns. The
  effect state header already carries `slot_id` at `[0]`.

### 5.2 What

Per probe per block, the wrapper writes:

- **Summary:** `last`, `min` and `max` over the block, stored as three
  `AtomicU32` (f32 bits). Tearing between the three fields is acceptable
  for display. `number` and `meter` read the summary.
- **Scope ring:** for `@view scope`, store the block decimated to a fixed
  number of points per block (min/max pairs, so peaks survive) in a preallocated
  single-producer ring. The UI reads the most recent window.

No allocation on the audio thread. Probe slots and rings are allocated on the
main thread when the instance is built (where `set_dgen_instrument_amp_channel`
runs today) and published to the audio thread through the registry's existing
`Release`/`Acquire` pattern.

### 5.3 Which voice

Each probe has a value per voice. v1 displays the **most recently triggered
voice** of the instance, which the engine pool already knows when it allocates
a voice. Only that voice's block feeds the summary and ring, so the cost of
capture doesn't grow with the number of voices. Follow-up: a per-probe
max-across-voices mode for envelope-like signals.

### 5.4 Gating

A global `PROBES_WATCHED` counter, raised by the UI while a patcher with live
probes is visible. When it's zero, the wrapper skips capture. The compiled
probe still writes its output buffer, which costs only the memory traffic.

## 6. UI

### 6.1 Publishing

`metal_seq`'s live audio analyzer (the band-meter path in
`live_audio_analyzer.rs`) collects the probes of the visible patcher's target
instance. It reads the summaries and rings, and publishes an
`eseqlisp::live_audio::ProbeFrame` keyed `probe:<instance>:<id>#<occurrence>`.
It publishes only when a value moves past an epsilon, so a static patch doesn't
redraw.

Instance resolution: the patcher already edits a specific track's instrument
or effect (the screenshot shows the track instrument below the canvas). That
track's engine or effect node is the instance. A patcher opened with no live
instance shows probes as `—`.

### 6.2 Patcher nodes

`probe` is an ordinary builtin node in the patcher. It writes source, and
writeback round-trips it like any other node. What makes it special:

- **Creation:** typing `probe`, `number~` or `scope~` in the new-object box
  creates `(probe <in> @id "<generated>" @view number|scope)`. The `~` names
  are patcher aliases for the view and are never written to source. The
  generated `@id` is unique within the source and stays stable across edits.
- **Insert on a cable:** right-clicking a cable and choosing "Insert Probe" or "Insert Scope" splices a
  passthrough probe in. With nothing connected to its outlet, a probe is a
  dangling top-level form (`should_emit_top_level` must accept it).
- **Mapping:** node ↔ manifest entry by `@id`. In a macro view, a node shows
  occurrence 0. In the root view, a macro instance node doesn't show the
  probes inside it in v1.
- **`number` view:** the node box shows the live `last` value instead of
  (or after) its label, formatted to about 4 significant digits. Hovering shows
  min/max for the block.
- **`scope` view:** a taller node that draws the ring as a waveform, with an
  auto-ranging y axis and the current min/max labelled. Its size is stored in
  the layout sidecar (`dsp.layout.json`) like node positions.
- **`meter` view:** a bar showing the summary between auto-tracked min and max.
  Optional in v1.

## 7. Non-goals (v1)

- Tensor and batched probes.
- Probing eseqlisp (non-DGen) graphs.
- Recording or exporting probe data.
- Probes as modulation sources. Routing a probe value elsewhere is `out`'s job.

## 8. Follow-ups

- Tensor probes (one channel per element up to a cap, or a reduced view).
- Max-across-voices and per-voice overlay views.
- Probe values shown in the code editor as inline widgets
  (see `inline-code-widgets-spec.md`).

## 9. Slices

| Bead | Slice | Depends on |
|------|-------|-----------|
| .1 | dgen: `probe` primitive, channel assignment, manifest `probes[]`, `--no-probes`, README; release + lock bump | — |
| .2 | eseq: manifest parse + unified non-audio channel set (routing, effect chain, buffer sizing, audition tools) | .1 (testable earlier with hand-written manifests) |
| .3 | Audio-thread capture: summaries + scope ring, instrument and effect wrappers, last-triggered voice, `PROBES_WATCHED` gate | .2 |
| .4 | UI publish: `ProbeFrame` + analyzer collection for the visible patcher's instance | .3 |
| .5 | Patcher `number` view + probe creation (`probe`/`number~`, generated `@id`, dangling top-level emit) | .4 |
| .6 | Patcher `scope` view (resizable, sidecar size) + insert-probe-on-cable | .5 |
| .7 | Manual: probes section in the patcher docs | .5 |
