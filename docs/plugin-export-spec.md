# Plugin export: an eseq instrument as a VST3 / CLAP / AU plugin, with its panel

Status: rev 1, 2026-10-30. Plan only; nothing built. Epic: see §13.

## 1. Goal

From inside eseq, **Export Instrument as Plugin…** on any DGenLisp instrument
(first targets: Latent Kick, Latent Kick Morph, Latent Snare, Latent Snare
Morph) produces plugin bundles that load in a DAW on the same Mac:

- the instrument sounds identical to eseq (same compiled DSP, same voice
  semantics, bit-for-bit for the same note/param stream — §10 parity gate);
- the DAW window shows **the instrument's own `ui.lisp` panel**, rendered by
  the same eseqlisp/Metal renderer, the same theme, the same family map —
  not a generic knob list;
- every visible param is host-automatable, saved with the DAW session, and
  re-exporting the instrument updates the plugin in place (stable IDs).

Non-goals for v1: Windows; Intel Macs; exporting racks, effects or whole
tracks; p-locks/key-locks/macros/process lanes inside the plugin; AUv3;
plugins that compile DSP at load time.

## 2. What exists (findings, 2026-10-30)

**DSP is already plugin-shaped.** `dsp.lisp` compiles through the pinned
DGenLisp + hermetic clang/lld stage (both shipped inside ESeq.app,
`docs/release-packaging-spec.md` §4.4) to a self-contained dylib importing only
libm/libc, ABI `cs/audiograph/dgen_abi_v1.h`:

```c
void dgen_process_v1(const float*const* in, float*const* out, uint32_t nframes,
                     void* state, const DGenProcessContextV1* ctx,   // ctx carries sample_rate
                     const DGenHostServicesV1* host);                 // FFT table, NULL ok if non-spectral
```

- All state is one float span (`totalMemorySlots`; Latent Kick ≈ 755k floats
  ≈ 3 MB per voice, weights included). Params are cells (`manifest.params[].cellId`).
  `@file` weight assets (`w1.json`…) are baked into the manifest's
  `tensorInitData` at compile time — **no runtime asset I/O**.
- The DSP is single-voice. Voice allocation, gate/pitch/velocity/trigger
  signal generation (`effects/gatepitch.rs`) and the 4-slot modulator
  (`instruments/voice_modulator.rs`) are Rust, wired per-voice in
  `app/graph/node_build.rs:714-889` and entangled with track/rack/global
  atomic tables (`instrument_storage.rs:1992-2189`).
- **Hazard:** generated C keeps file-scope scratch statics
  (`static float tN_g[VOICE_COUNT * SCRATCH_STRIDE]`, `SCRATCH_STRIDE = 512`).
  Two instances sharing an image race; >512 frames per call overruns. eseq
  works around it by cloning the dylib per live instance (eseq-599).
- Best prototype of a plugin voice: `render_loaded_instrument_samples_for_test`
  (`lisp_host/dgen/instrument_compile.rs:484-688`) and `bin/instrument_probe.rs`.

**The UI renderer is already a library.** `crates/eseqlisp` (VM, reactive
runtime, layout, widgets incl. `family-map`, Metal + wgpu backends) has no
sequencer dependency. What ties a panel to the app is the *Lisp binding layer*
(`content/ui/effects/custom-ui-runtime.lisp`, `custom-ui-lego.lisp`,
`param-controls.lisp`, `eseq.kinds`) and its data publishers
(`ui/host_kinds/*`, `ui/host_commands/*`), which live in the metal_seq binary.

- One-instrument panel codegen: `ui/custom_ui.rs:440-638`
  (`build_custom_instrument_ui_source_with_overlay`; ~200 lines, parser-only).
- Host-less panel harness that already boots a real panel with no App:
  `ui/state_values/kick808_ui_tests.rs` + `panel_kinds_seed.rs` (`seed_panel_kinds`).
- Renderer with no window: `MetalBackend::new_capture` + `initialize_offscreen`
  (`eseqlisp/src/ui/metal_backend.rs:2753,2773`); the window path attaches its
  private `CAMetalLayer` to an NSView at `:7464-7474` — the seam a host NSView needs.
- Event loop is winit 0.29, which cannot live inside a DAW (owns NSApp, one
  loop per process, fixed ObjC class names).
- Latent panels need only param values back from the host; the family-map
  preview is CPU-side from `data/family.json`. Writes are
  `host-command "set-instrument-param"` / `"set-instrument-param-batch" {:commit}`.

**Packaging.** Developer-ID signing + notarization of the app works
(`dist/macos/build.sh`, `build-signed-binary.sh`). The shipped toolchain is
SDK-less: it can build DGen dylibs on the user's machine but **cannot link a
plugin binary** (no AppKit/Metal/Rust runtime). Targets: arm64 macOS,
x86_64 Linux only. License: GPL-3.0-only.

**Ecosystem (late 2026).** nih-plug is in maintenance mode; its community
fork **nice-plug** (ISC) is active, uses the MIT `vst3` crate (VST3 SDK is MIT
since 2025-10-29, so no GPL `vst3-sys`), baseview editors, CLAP + VST3, and
`nice-plug-au2` (new, 0.1.x) for AUv2. clap-wrapper is the fallback for AU.

## 3. Architecture: a prebuilt shell + a per-instrument payload

The decisive constraint: the user's machine can compile DSP but not link a
plugin. So the plugin binary is **built once at release time** and the
exporter only assembles bundles.

```
ESeq.app/Contents/Resources/plugin-shell/
  eseq-plugin-shell.dylib      # Rust cdylib: CLAP + VST3 (+ AUv2) entry points,
                               # instrument engine, embedded eseqlisp panel host.
                               # Developer-ID signed at release.
  ui-runtime/                  # content subset the panels need (§7.3), fonts

Export "Latent Kick" →
~/Library/Audio/Plug-Ins/CLAP/eseq Latent Kick.clap/
~/Library/Audio/Plug-Ins/VST3/eseq Latent Kick.vst3/
~/Library/Audio/Plug-Ins/Components/eseq Latent Kick.component/
  Contents/
    Info.plist                 # bundle id, AudioComponents (AU), version
    MacOS/eseq Latent Kick     # copy of eseq-plugin-shell.dylib
    Resources/
      moduleinfo.json          # VST3
      eseq-payload/
        plugin.json            # identity, params table, voice config, ABI/version stamps
        dsp/<name>.dylib       # the compiled DSP (from the dylib cache)
        dsp/<name>.json        # manifest incl. tensorInitData
        ui/panel.lisp          # generated one-instrument panel source (§7.2)
        ui/runtime/…           # copy of ui-runtime/ (or a hardlink-free copy; bundles must be self-contained)
        instrument/            # data/family.json and any other UI-read files
        presets.json           # factory + user presets of this instrument
        source/                # dsp.lisp, ui.lisp, eseq commit — GPL corresponding source
```

One shell binary serves every exported instrument; it reads its payload
from its own bundle path at load (CLAP `init(plugin_path)`; VST3/AU via
`dladdr` on a shell symbol → bundle). Each exported plugin is a distinct file
path, so dyld maps a distinct image per exported instrument: process-wide
statics (Rust `OnceLock`s, eseqlisp asset roots) are per-*plugin*, shared only
between instances of the same plugin — which all want the same values.

Rejected alternatives:
- *Link DSP statically into a freshly built plugin at export*: impossible with
  the SDK-less stage; would also make every instance share scratch statics.
- *Ship the DGenLisp compiler in the plugin, compile at load*: 140 MB per
  bundle, slow first load, compile errors surfacing inside a DAW.
- *Generic knob UI first*: explicitly out — the panel is the point.

## 4. Crates

| Crate | Content | Depends on |
|---|---|---|
| `crates/dgen-host` (new, lib) | ABI types, dylib loading, manifest parse, init-state build, FFT host services, gatepitch signal logic, voice modulator DSP, voice pool, block splitter, param store. **Extracted from `sequencer`, which then depends on it** — one copy of each, no dual maintenance. | libc, rustfft (non-mac) |
| `crates/eseq-plugin` (new, cdylib) | nice-plug CLAP/VST3 (+AU) shell, payload loader, state save/restore, presets, editor (baseview + eseqlisp) | dgen-host, eseqlisp, nice-plug |
| `crates/eseqlisp` (existing) | + embed API: render into a host NSView, injected input, window-less text measurer/cursor (§7.1) | — |
| `crates/sequencer` (existing) | + exporter (§9), uses dgen-host | dgen-host |

The plugin must never link `sequencer` (cpal, aubio-GPL vendor, rusqlite,
reqwest, the audiograph C build). Extraction cost lands mostly in §5's
voice-pool/modulator split from `audio/voices/*` and the registry statics.

## 5. Audio engine (`dgen-host`)

Per plugin instance:

- `Engine { voices: [Voice; N], params: ParamStore, modulator config, sr }`,
  N = export-time polyphony (default 16; drums may export 8 to cut memory —
  Latent Kick is ~3 MB/voice).
- `Voice { memory: Box<[f32]> (totalMemorySlots + 256 redzone), gatepitch
  state, modulator state, age, released, amp_active }` — all allocated at
  `activate`, never on the audio thread.
- `process(host_block)`: split into ≤512-frame chunks **and** at every event
  (note, sample-accurate param change) — the `render_loaded_instrument_samples_for_test`
  loop, generalized to N voices; sum voice outputs to the stereo bus.
- Notes: MIDI/CLAP note on/off → allocator (poly with oldest-release steal,
  mono/legato per `instrument.json voice_controls.mode`, ported from
  `voices/pool.rs` / `mono.rs` / `legato.rs`) → gatepitch signals: pitch Hz,
  velocity 0..1, gate, trigger pulse (not on legato), pitchbend, modwheel,
  pressure (inputs matched by name, `host_signal_output_for_input`).
  CLAP note IDs/note-end events supported; MPE later.
- Voice retirement via the `@amp` output (`ampOutput`) + release timeout;
  report CLAP tail / VST3 `getTailSamples` from tail-sleep metadata.
- Params: host value → clamp → write `memory[cellId]` on every active voice
  (and on idle voices at note-on). `@mod` active cells maintained by the host
  exactly as eseq does (`docs/…/dgenlisp-modulation-manifest-migration.md`).
- Sample rate: `ctx.sample_rate` per call + the host-SR cell; on SR change
  re-run init (`activate`). Spike must confirm no SR is baked at compile time.
- Voice modulator: v1 runs the 4 slots with the configuration snapshotted
  from the exporting track (so a sound that relies on an LFO keeps it); it is
  not editable in the plugin in v1 (§12). Bar-phase sync comes from host
  transport (CLAP transport / VST3 ProcessContext).
- Multi-instance safety: see §6.
- RT safety: no alloc/lock/IO in `process`; extend `bin/audio_rtsan_probe.rs`
  to drive the shell's process path.

## 6. The scratch-static problem

Instances of one plugin share one image → shared `tN_g` statics → DAWs render
tracks on parallel threads → corrupted audio.

- **Fix (DGen, required before v1 ships): `--scratch in-state`.** Codegen
  places scratch at a manifest-declared offset in the per-voice state span
  (`scratchCellOffset`, `scratchCells`), no mutable file-scope data. ABI stays
  `dgen_process_v1`; the manifest gains a flag; `dgen_audit` gains a "no
  writable data section" check for in-state builds. eseq itself can then drop
  the eseq-599 per-instance dylib clones (bonus: less disk, faster instance
  creation). Cost to measure: scratch now per-voice in heap memory instead of
  a contiguous static; expected negligible.
- **Interim (spike only):** the shell clones `dsp/<name>.dylib` to
  `~/Library/Caches/com.universalsequences.eseq/plugin-instances/<id>/<n>.dylib`
  and dlopens the copy, as eseq-599 does. Not shippable: writes executables
  from inside the DAW and breaks with hardened hosts.

## 7. The panel in the DAW

### 7.1 eseqlisp embed API (renderer + input)

Add to `MetalBackend` (public, macOS):

- `MetalBackend::new_embedded(scale)` = `new_capture` + `initialize_offscreen`
  without winit; `attach_to_ns_view(*mut NSView)` (setWantsLayer/setLayer as
  at `:7464-7474`); `set_view_size(points, scale)` → `layer.setDrawableSize`;
- window-less `create_text_measurer` / `viewport_size` (today read `self.window`);
- `push_event(EmbedEvent)` feeding the existing `pending` / coalesced
  drag/move/scroll/magnify queues (the consumer side is unchanged:
  `editor.handle_mouse_precise`, `handle_key`, …);
- cursor + hidden-drag via NSCursor / `CGAssociateMouseAndMouseCursorPosition`
  instead of `winit::Window`.

The plugin's NSView comes from **baseview** (child view of the host's
NSView, randomized ObjC class names); its event callbacks are translated to
`EmbedEvent`s — a port of `handle_native_event` (`:7151-7397`) without the
titlebar/file-drop/close arms. Frames are driven by baseview's frame callback
(display-linked), rendering only when `editor.needs_redraw()` or an animation
is live (the `FramePacer` policy). `Runtime`/`Editor` are `!Send`: they live
on the GUI thread, created on `open`, dropped on `close`.

Keyboard: DAWs eat keys; v1 handles only what the panel needs (text entry in
number pickers) and otherwise passes keys back to the host.

### 7.2 One-instrument panel source

Move the parser-only part of `build_custom_instrument_ui_source_with_overlay`
(`ui/custom_ui.rs:440-638`: helper renaming, `(param x)` rewrite, the
`custom-synth-ui-<id>` wrapper) into a function both the app and the
exporter call with a single instrument folder. The exporter writes the result
to `ui/panel.lisp`; the plugin never parses `dsp.lisp`.

### 7.3 UI runtime subset

The latent panels reach, via `custom-ui-runtime`/`-lego`/`-sections`/
`param-controls`/`devices`/`state`/`macro-state`, into `eseq.kinds`
(`content/core/modules/kinds.lisp`), the theme files and JetBrainsMono.
Ship a curated **`content/plugin/ui-runtime.lisp`** root that loads exactly
that set (not all 26 files of `effects.lisp`), plus a check test that every
factory instrument panel evaluates against it with zero unresolved symbols.
Unavailable app features are inert stubs in that root, not missing symbols:
`seq-has-selection?` → false, macro-arm/rack-armed → false, p-lock colors →
neutral, mods-open → false.

### 7.4 Kinds publisher + command bridge (the plugin's tiny `host_kinds`)

- Publish one `device` and its `param` instances with eseqlisp's instance
  APIs, the way `seed_panel_kinds` does in tests — but live: on each GUI
  frame, diff the `ParamStore` (atomics written by audio/host automation)
  and `set_instance_field` changed `value`s, so DAW automation moves the knobs.
- Drain `host-command`s: `set-instrument-param {param-idx value}` and
  `set-instrument-param-batch {updates commit}` → `ParamStore` + host
  notification. Gesture mapping: pointer-down on a widget → CLAP
  `begin_gesture` / VST3 `beginEdit`; pointer-up (or `:commit true`) → end.
  Any other host command → logged once and ignored.
- Panel-local Lisp state (`defstate`, e.g. `lkm-ui-state` = which ring is
  selected) is serialized into plugin state (§8) via a small eseqlisp API to
  snapshot/restore named defstates.

### 7.5 Size, theme, scale

Size = panel layout extent in cells × font metrics, computed at export (and
re-checked on open); fixed-size in v1. Theme chosen at export (default: the
current eseq theme); Retina scale from the host view's backing scale.

### 7.6 Budget

Editor open → first frame ≤ 300 ms on an M-series Mac. Risk: evaluating
`kinds.lisp` + the effects subset is the heaviest part of eseq's startup.
Measure in the spike; mitigations if over: precompiled bytecode cache in the
payload, lazily loaded modules, keeping the `Runtime` alive across close/open.

## 8. Host-facing identity, params, state, presets

- **Stable IDs** are generated on first export and stored in the instrument
  folder's `instrument.json` (`plugin: {clap_id, vst3_uid, au_subtype,
  au_manufacturer}`), so renames and re-exports keep DAW sessions loading.
  Default `clap_id = "com.universalsequences.eseq.<slug>"`, VST3 UID =
  first 16 bytes of sha256(clap_id), AU manufacturer `Eseq`, subtype from
  the hash. Factory instruments get IDs checked in.
- **Params:** every non-hidden manifest param, in manifest order. Param ID =
  stable hash of the param *name* (adding params doesn't renumber). Range
  from min/max/unit; `options` → stepped enum; `@mod` depth/active internals
  stay hidden. Display text uses eseq's unit formatting.
- **State (save/load):** JSON `{format: 1, plugin_id, dsp_sha256, params:
  {name: value}, preset_id, modulator: {...}, ui: {defstates}}`. Loading
  state from an older export: params matched by name, unknown names dropped,
  missing ones default; a `dsp_sha256` mismatch is fine (that's an update).
- **Presets:** the instrument's `.presets` bank (factory + user overlay) is
  frozen into `presets.json`; exposed as VST3 program list / CLAP
  preset-discovery and a preset picker strip added above the panel by the
  plugin root (Lisp, reusing the app's preset picker widget).

## 9. Exporter (in eseq)

Entry points: File › Export Instrument as Plugin…, the instrument header
context menu, and headless `eseq_export plugin <instrument-id> [--formats
clap,vst3,au] [--out DIR] [--voices N] [--theme NAME]` (for tests/CI).

Steps:
1. Resolve instrument + release (instrument versioning ids); snapshot the
   track's current param values as the plugin default (option: factory defaults).
2. Ensure compiled with `--scratch in-state --voices 1 --max-frames 512`
   (cache hit when unchanged); audit; copy dylib + manifest.
3. Generate `ui/panel.lisp` (§7.2); copy ui-runtime, fonts, instrument data
   files the panel references (`:file` args resolved through the asset
   resolver — collect by evaluating the panel once headlessly and recording
   resolved asset paths); copy presets, source.
4. Write bundles per format from `plugin-shell/`: binary copy, Info.plist,
   moduleinfo.json (VST3), AudioComponents (AU, `aumu`).
5. Sign ad-hoc with `/usr/bin/codesign --force --sign -`, inner dylibs first
   then the bundle (no `--deep`). Optional: user-supplied Developer ID + notary
   profile fields for sharing with other machines.
6. Validate out of process: `eseq-plugin-check <bundle>` (shipped helper) loads
   the CLAP, opens state, renders a note at 48 kHz and checks non-silent,
   finite output and that the editor builds offscreen. A crash never takes
   eseq down.
7. Install (default `~/Library/Audio/Plug-Ins/<format>/`), reveal in Finder,
   and remind: rescan plugins in the DAW (AU: `killall -9 AudioComponentRegistrar`
   is offered as a button).

Export modal: name, formats, polyphony, theme, defaults (current/factory),
install location, sign identity (advanced). Progress per step; errors name
the step.

## 10. Testing and gates

- **Parity (gate for every slice touching DSP):** same note/param event
  stream through `dgen-host` and through `instrument_probe` → identical samples
  (both run the same dylib; any difference is a host-logic bug).
- `dgen-host` unit tests: chunking at 512 and at events, voice stealing,
  legato, @amp retirement, SR change, state round-trip.
- Two-instance concurrency test: two engines of the same in-state dylib on
  two threads, outputs equal to single-instance renders.
- Panel: headless test boots `content/plugin/ui-runtime.lisp` + each factory
  latent `panel.lisp`, renders offscreen (render-panel style), clicks a knob,
  asserts a `ParamStore` write and gesture begin/end.
- Format conformance: `clap-validator`, `pluginval` (VST3/AU), `auval -v aumu`.
- Manual host matrix per release: Bitwig (CLAP), Reaper (CLAP/VST3/AU),
  Ableton Live (VST3/AU), Logic (AU). Two instances, automation, save/reload,
  close/open editor repeatedly, SR change 44.1/48/96, buffer 32…2048.

## 11. Platforms, signing, license

- v1: macOS arm64. Linux (CLAP/VST3, X11 embed via baseview, wgpu raw-handle
  surface) is a follow-on; Windows needs a DGen Windows target first.
- Release: `dist/macos/build.sh` builds + Developer-ID signs `eseq-plugin-shell.dylib`
  and `eseq-plugin-check`. Export rewrites the bundle, so the result is
  ad-hoc signed. That is enough on the exporting Mac for hosts that load
  third-party plugins (no quarantine on locally written files). Sharing with
  another Mac needs the recipient to clear quarantine, or the optional
  user-identity signing + notarization in §9.5. **Verify Logic's AU loader
  accepts ad-hoc signed `.component` bundles in the spike.**
- License: exported plugins contain GPL-3 eseq code → distributing one means
  offering source; the payload's `source/` + eseq commit hash covers it.
  Whether to add a plugin-export linking exception for owned code is a
  separate decision (§12). Don't use the VST logo without following
  Steinberg's trademark guidelines; naming "VST3" in text is fine.

## 12. Open decisions (recommendations in bold)

1. Formats in v1: **CLAP + VST3 first, AUv2 in the same epic as a later
   slice** (Logic needs it; nice-plug-au2 is young, clap-wrapper is the fallback).
2. Mod slots in the plugin: **snapshot-only in v1**; editable mod editor later.
3. Default sound: **current track values**, with factory as an option.
4. Polyphony: **16, 8 for Drums category** (memory: weights are per voice —
   see follow-up on shared read-only tensors).
5. Sharing beyond this Mac: **out of v1 scope**, but keep the signing hook.
6. License exception for exported plugins: owner's call; not blocking.

## 13. Plan (beads under `eseq-plug`)

Spikes de-risk the three unknowns first; nothing else starts before they pass.

| Bead | Slice | Exit |
|---|---|---|
| .1 | **Spike A — sound in a DAW.** Throwaway CLAP shell (nice-plug) hard-pointed at a cached Latent Kick dylib + manifest; per-instance dylib clone; MIDI → 1..N voices | Kick plays in Bitwig/Reaper, two instances OK, SR change OK; notes on SR baking |
| .2 | **Spike B — panel in a DAW.** eseqlisp Metal layer attached to a baseview NSView in the spike shell; Latent Kick panel booted with kick808-harness-style stubs; knob drags print param writes | Panel draws + responds in Bitwig & Reaper; open-time measured vs 300 ms |
| .3 | **DGen in-state scratch** (dgen repo + vendor bump): `--scratch in-state`, manifest offsets, audit check; eseq uses it and drops per-instance clones | Concurrency test green; eseq parity + perf unchanged |
| .4 | **`dgen-host` crate extraction**: ABI/manifest/init/FFT/gatepitch/modulator/voice pool/chunker; sequencer switched to it | Parity test vs `instrument_probe`; sequencer suites green |
| .5 | **eseqlisp embed API** (§7.1) | Embedded render + injected input test; metal_seq unchanged |
| .6 | **Panel source + UI runtime subset** (§7.2, §7.3): shared codegen fn, `content/plugin/ui-runtime.lisp`, stubs | All factory panels evaluate headlessly against the subset |
| .7 | **Plugin shell v1** (CLAP + VST3): payload loader, params, notes, state, presets, kinds publisher, command bridge, gestures | clap-validator + pluginval pass on a hand-built Latent Kick bundle |
| .8 | **Exporter core + CLI** (§9 steps 1–7, `eseq_export plugin`) | CLI exports all four latent instruments; check helper passes |
| .9 | **Export modal + menu entries** | Export from UI end to end, installed, loads in a DAW |
| .10 | **Release integration**: shell + check helper built/signed in `dist/macos/build.sh` | Signed DMG contains shell; export from installed app works on a clean account |
| .11 | **AUv2** | `auval` passes; loads in Logic |
| .12 | Follow-ups (filed, unscheduled): shared read-only tensors across voices (DGen); editable mod slots; resizable panel; MPE; audio-effect export (same pipeline, `run_mode effect`); Linux CLAP/VST3 | — |

Dependencies: .1,.2 → everything. .3 → .7 ship. .4 → .7. .5 → .7. .6 → .7.
.7 → .8 → .9 → .10. .7 → .11.
