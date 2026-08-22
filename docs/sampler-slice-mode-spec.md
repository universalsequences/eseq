# Sampler Slice Mode Spec

Status: draft / design
Author: design pass, 2026-08-22
Related: `crates/sequencer/src/analysis.rs` (warp onset analysis, the reused
engine), `docs/drum-rack-v2-spec.md` (explicitly NOT the mechanism, see §2.4),
`docs/key-locks-spec.md` (sibling "note selects behavior" idea),
memory: sampler start/end live-update fix (stale pool slot descriptor).

## 1. Goal

Give the built-in Sampler a native **slice mode**, modeled on Ableton Simpler's
Slice mode: the sample is auto-sliced at transients, and **note input selects a
slice** instead of repitching the sample. Playing C4 fires slice 0, C#4 fires
slice 1, etc. — live keyboard, sequenced steps, and step-sequencer note lanes
all become slice selectors for free.

This replaces the current workaround of sequencing `start`/`end` p-locks per
trigger, which is not playable live, re-encodes the same chop in every pattern,
and breaks when the sample changes.

Design center: **one simple sampler that is aware of slices.** No new
instrument, no multi-track routing, no per-slice fx chains.

## 2. Locked decisions

1. **Slice resolution happens at trigger time, in the existing param-resolve
   seam.** The three sites that assemble `(start_point, end_point, transpose)`
   into `ScheduledSamplerParams` before `send_trigger` /
   `send_keyboard_trigger` — sequenced (`audio/fire.rs`), live keyboard
   (`audio/callback.rs:275`), rack member (`audio/rack.rs`) — call one shared
   helper: when slice mode is on, map `transpose` → slice index →
   `(start, end)`, then force `transpose = 0`. The DSP core
   (`instruments/sampler.rs`) is unchanged; a slice trigger is
   indistinguishable from a trigger with those start/end values.
2. **Notes beyond the slice table are ignored** — no trigger fires. Same
   semantics as an unmapped drum-rack pad note. No wrap, no chromatic-repitch
   of the last slice (v1; see §9).
3. **Slice N's end = slice N+1's start.** Only slice *start* positions are
   stored. The last slice ends at the sample end. Markers can never overlap or
   leave gaps, and dragging one marker adjusts two slices consistently.
4. **Slice mode does not route through drum-rack machinery.** A rack pad
   resolves to a whole member track with its own instrument + fx chain; a
   slice resolves to a sub-region of one sample inside one voice. The two
   compose (a rack member sampler may itself be in slice mode) but share no
   code. What we *do* borrow is the mental model: base note C4 = transpose 0,
   ascending chromatically, same note-domain clamping conventions.
5. **The slice table is per-sample derived data plus user overrides, stored
   like the warp onset table — not as scalar params.** Auto-detected slices
   come from the already-computed aubio onset analysis
   (`analysis.rs::analyze_with_aubio`, published to the audio thread via
   `publish_sampler_analysis_runtime` and a packed pointer). Manual edits are
   a sparse override layer serialized with the instrument slot (§5). This
   avoids growing the scalar param descriptor with an unbounded list — the
   descriptor-growth path is exactly the stale-pool-slot mismatch class fixed
   in `effects/mod.rs:9372` — and matches the existing warp precedent.
6. **P-locks still win.** Resolve precedence per trigger, per param:
   patch default → **slice-resolved start/end** → explicit step p-lock on
   start/end → modulation on top. A step p-lock is the more specific gesture;
   this preserves the existing micro-edit workflow on top of slices.
7. **Auto-detected slices refresh with the sample; manual edits are anchored
   to the sample.** Loading a different sample into the slot discards manual
   slice edits (they are meaningless against other audio) and re-derives from
   that sample's analysis.

## 3. Data model

### Scalar params (added to the sampler descriptor)

Only small scalars join the descriptor — few enough to keep the
`apply_authoring_values` migration surface trivial, but the old-project
descriptor-length mismatch path must be verified (regression test exists:
`sampler_range_edit_survives_stale_pattern_pool_descriptor`,
`app/edit.rs:10343`).

| param | type | default | meaning |
|---|---|---|---|
| `slice` | enum {off, transient, division} | off | slice mode + detection source |
| `sens` | 0..1 | 0.5 | transient sensitivity: filters the aubio onset list by strength / min-spacing |
| `div` | enum {1/4 … 1/32} | 1/16 | grid division when `slice = division` (uses analysis BPM + downbeat) |
| `slice base` | note | C4 | note that fires slice 0 |

`slice`, `sens`, `div` are p-lockable like any scalar but are expected to be
patch-level in practice. Per-slice playback shaping (1-shot vs gate, choke) is
out of scope for v1 — the existing global `gate`/`loop`/envelope params apply
to every slice uniformly.

### Slice table

```
SliceTable {
    source: Transient { sens } | Division { div },
    detected: Vec<u32>,          // frames, derived from AnalysisResult.onsets_frames
    user_added: Vec<u32>,        // frames
    user_deleted: Vec<u32>,      // detected onsets the user removed
    user_moved: Vec<(u32, u32)>, // detected onset → new position
}
resolved(): sorted, deduped Vec<u32> — always begins at frame 0
```

Runtime home: an `Arc<SliceTableShared>` cached next to `AnalysisCache`,
delivered to the trigger-resolution sites the same way the warp onset table
reaches the sampler (`pack_ptr` / two aux f32 slots, `analysis.rs:131`).
Resolution happens on the trigger path *before* `send_trigger`, so the packed
pointer is read control-side, not inside the DSP render loop.

## 4. Trigger-time resolution

```
fn resolve_slice(params: &mut ScheduledSamplerParams, table: &SliceTable,
                 base_note_transpose: i32) -> TriggerVerdict {
    let idx = params.transpose_semis - base_note_transpose;
    let slices = table.resolved();
    if idx < 0 || idx as usize >= slices.len() { return TriggerVerdict::Ignore; }
    params.start_point = frame_to_norm(slices[idx]);
    params.end_point   = frame_to_norm(slices.get(idx + 1).copied()
                                       .unwrap_or(sample_len));
    params.transpose_semis = 0;
    TriggerVerdict::Fire
}
```

- Applied identically on all three trigger paths (§2.2); called after
  p-lock/default resolution but explicit start/end p-locks re-override (§2.6).
- `Ignore` suppresses the trigger entirely (no voice allocated).
- `[srange]` tracer (`srange_debug_enabled()`) is extended to log slice
  resolution: `note → idx → (start, end)` — primary bring-up tool.

## 5. Serialization

- Scalar params: normal descriptor serialization (`defaults` /
  `plocks` in `EffectSlotSnapshot`). Old projects deserialize with the
  shorter descriptor → new params take defaults (`slice = off`), so existing
  projects are bit-identical in behavior.
- User slice edits (`user_added` / `user_deleted` / `user_moved`): a new
  optional field on the instrument slot's project representation, keyed
  implicitly by the slot's `sample_path` (content-addressed hash). If the
  sample hash at load time doesn't match, edits are dropped (§2.7).
- `detected` is never serialized — recomputed from analysis on load, like
  warp's onsets.

## 6. UI

Extends the existing waveform widget
(`crates/eseqlisp/src/widget_render/waveform.rs`) and sampler panel
(`content/ui/effects/sampler-panel.lisp`); no new widget.

1. New waveform props: `:slices` (list of times), `:active-slice` (index of
   the last-fired / selected slice). Rendered as vertical markers with top
   flags, next to the existing marker pass (~`waveform.rs:309`); active slice
   region gets the highlight treatment currently used for the start/end
   selection.
2. New actions in the existing `WidgetEvent::Custom` channel: `:add-slice`
   (double-click / modifier-click), `:move-slice` (drag a marker),
   `:delete-slice` (modifier-click a marker). Handled in
   `handle-sampler-waveform-action`, issuing host commands with a
   `"sampler-slice"` gesture for undo coalescing (history-host-command
   pattern, no `ui_epoch` bump).
3. Panel row: `slice` mode selector, `sens` knob (transient), `div` selector
   (division), `slice base` — appearing in the existing param strip when
   slice ≠ off. Start/end fields stay visible and reflect the *last fired*
   slice.
4. When slice mode is on, start/end drag handles remain functional (they now
   edit the global sample window that slicing subdivides — v1: slices span
   the full sample; window-clipping is a possible refinement).

## 7. Interactions with existing features

- **Warp**: orthogonal. Warp changes playback speed mapping; slices change
  which region fires. Both read the same analysis. Allowed simultaneously.
- **Key locks**: compose naturally — the sounding note picks the slice, key
  locks may still shape other params by note. Both resolve at
  voice-assignment time; slice resolution consumes transpose, key locks read
  the note before it is zeroed.
- **Record / takes / arrangement**: nothing special — slices are patch data,
  sequences store plain notes.
- **Rack member samplers**: slice mode works inside a rack pad's sampler;
  the pad note routes to the member track first, then the member's own
  keyboard transpose (if any) selects the slice.

## 8. Implementation slices (beads)

1. **Core resolve + transient table** — `SliceTableShared` derived from
   `AnalysisResult.onsets_frames` with `sens` filtering; scalar params added
   to descriptor; shared `resolve_slice` wired into all three trigger paths;
   `[srange]` extension; descriptor-migration regression coverage.
2. **Waveform UI** — `:slices` / `:active-slice` props, marker render pass,
   add/move/delete actions + undo; panel param row.
3. **Manual edits + serialization** — user override layer, project
   round-trip keyed by sample hash, drop-on-sample-change.
4. **Division slicing** — grid slices from analysis BPM/downbeat; `div`
   param.

## 9. Out of scope / future

- Per-slice playback params (1-shot vs gate per slice, per-slice pitch/gain).
- Chromatic-repitch of a selected slice above the table (Ableton offers this;
  our answer today is "use classic mode with start p-locks").
- "Explode slices to drum rack" command (each slice → a rack pad with its own
  chain). High-value follow-up, deliberately separate.
- Slicing within the start/end window rather than the full sample.
