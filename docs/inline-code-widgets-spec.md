# Inline Code Widgets: Strudel-Style UI in the Buffer

Status: design spec (draft). Scheduled after the Cirklon step-processes work
(`docs/cirklon-process-accumulator-brainstorm.md`) lands — this feature is what
makes the process code-prototyping phase actually usable, but it depends on
process inlet metadata and the handle write-through path being settled first.

## The Idea

Strudel renders live UI *inside* the code buffer: `._scope()` injects a
full-width oscilloscope band between two source lines (line numbers stay
honest — 6, scope band, 7), `slider(...)` puts a draggable control next to the
literal it wraps, and active pattern tokens get boxed highlights. The code is
the instrument panel.

ESeq today forces a binary choice: write a full `ui.lisp` that maps widgets to
some other buffer, or stay in plain code. The hybrid — code as the primary
surface, with widgets anchored to the expressions they control — is the missing
prototyping mode. Concretely:

```lisp
(processes :track 0
  (transpose-climb :limit (~slider 12)             ; slider next to this literal
                   :delta (lane 0 1 0 0 1 0 0 0))) ; lane band under this line,
                                                   ; with accumulator curve preview
(~scope :track 0)                                  ; scope band after this line
```

`(~slider 12)` evaluates to `12` (range inferred from the inlet's `:in`
metadata); as a side effect of buffer eval it
registers a widget anchored to its own source span. Dragging the slider writes
the new value back into the buffer text.

## Verified Repo Facts (2026-07-08)

The source-mapping half of this feature already exists; these ground the plan:

- **Parser tracks byte spans for every expression.** `SourceSpan` at
  `crates/eseqlisp/src/lang/parser.rs:26` (`start_byte`/`end_byte`), wrapped in
  `SourceOrigin` with an `expansion_chain` so spans survive macro expansion.
- **Eval already stamps spans onto widget forms.** `convert_source_expr`
  (`lang/vm.rs` ~1020, `annotate_widgets` path) injects
  `__source-start-byte` / `__source-end-byte` / `__source-revision` props into
  widget expressions; `__source-buffer-id` / `__source-module-path` are added
  nearby (vm.rs ~1411). Constants at `lang/vm.rs:15`.
- **The editor already reads them back.** `inspect_node_source_span` /
  `inspect_node_source_buffer_id` etc. at `editor/mod.rs:200–242` — currently
  used for the widget→code direction (inspect/jump-to-source). Nobody runs the
  arrow the other way (code position → screen position at render time).
- **Buffers already render text + widgets simultaneously.**
  `ViewMode { Both, UiOnly, TextOnly }` at `editor/mod.rs:305`; `Both` is the
  default (`buffer.rs:158`). But `apply_view_mode` (`ui/frame.rs:1189`) just
  composites the widget tree over the text from the top-left — the two layers
  are independent, which is the exact problem this spec fixes.
- **Frame assembly is line-window based.** `ui/frame.rs` builds
  `RenderFrame.lines` from `scroll_top..scroll_top+viewport_height` buffer
  lines; cursor is `(cursor_row - scroll_top, cursor_col)`; the frame carries
  `widget_layout`, `text_scroll_top`, `widget_scroll_top` separately.
- **Buffer is line-based with a revision counter.** `Buffer` (`buffer.rs:49`)
  holds `lines: Vec<String>`, `revision: u64`, per-buffer
  `widget_tree` + `committed_ui_snapshot` (the reactive subtree machinery), and
  `text_styles: Vec<BufferTextStyle>` (per-line/range styling — the future seam
  for Strudel-style token highlights). `Buffer::text()` joins lines; byte spans
  from eval index into that joined string. **No marker/anchor system exists** —
  spans go stale the moment the text is edited.
- **Widget interaction inside buffers works.** `editor/widget_interaction.rs`
  and `editor/widget_focus.rs` handle mouse + focus for buffer-owned widgets.
- **Band content widgets already exist.** `widget_render/` has `hslider`,
  `knob_number`, `toggle`, `dropdown` for value rows and `waveform`,
  `spectrogram`, `live_audio` for scope-style bands.
- **Process-side hooks (from the Cirklon spec, Phases 0–3A landed):** inlets
  carry `kind/min/max/default/lane` metadata; instance handles support live
  knob tweaks (`(climb :limit 6)`) and `lane!`; ports have binding hints and
  the Phase 3B arm-mapping UI is the shared mapping seam.

## Core Architecture

### The gating decision: display-row map

Everything hangs off one structure, built per frame in `ui/frame.rs`:

```text
band:        (after_buffer_line, node_id, height_cells)
display map: buffer line  <->  display row   (bands shift subsequent lines down)
```

- A widget anchored after buffer line N reserves `height_cells` display rows;
  every later buffer line shifts down. Line numbers render from **buffer
  lines**, never display rows — that is what keeps the gutter honest
  (Strudel's 6 → scope → 7).
- Everything that touches rows goes through the map: cursor rendering, mouse
  hit-testing (click in a band → existing widget-interaction path; click on
  text → inverse-map to buffer line), and scrolling (scroll in display rows so
  a tall band scrolls smoothly past; viewport fits fewer text lines while
  bands are visible).
- Two placements, one mechanism:
  - **Band** (full-width row(s) after the anchor line) — scopes, lane
    mini-sequencers, accumulator-curve previews. The Strudel look.
  - **Margin** (right of the anchor line, zero extra rows) — sliders, knobs,
    toggles, binding chips. A degenerate band of height 0 with an x-offset;
    do not build it as a separate system.

This subsumes the cheap "right-margin only" version; build the map first.

### Span anchors that survive editing

`__source-revision` is a snapshot: any edit above a widget stales its byte
span, and the naive approach (hide on revision mismatch) makes every widget
vanish while typing — unacceptable mid-prototyping. Standard marker semantics
instead:

- Per-buffer anchor table: `anchor_id → (start_byte, end_byte, revision)`.
- Every text edit funnels through one notification seam on `Buffer` that
  shifts anchors (insert of N bytes at offset ≤ start → both += N; delete
  likewise; an edit *inside* the span marks the anchor dirty/stale).
- Audit required: edit entry points are spread across `editor/commands.rs` and
  `Buffer` methods; part of Phase 1 is funneling them through a single
  `apply_text_edit`-style seam. This is the riskiest mechanical work in the
  plan.
- Stale anchors dim their widget (still rendered at the adjusted position);
  re-eval refreshes spans and clears staleness. Byte→line conversion uses a
  line-start offset index rebuilt per `revision`.

### Authoring surface: `~` forms

`~` is the "inline" prefix — always followed by an explicit widget name; there
is no bare inferring `(~ v)` form (rejected: the most important information is
what you'll see). Value arguments are positional only for the value itself;
everything else is keywords:

```lisp
(~slider 12 :min 0 :max 24)   ; margin: horizontal slider; evaluates to 12
(~knob 0.5 :min 0 :max 1)     ; margin: knob
(~toggle 1)                   ; margin: toggle; evaluates to 1
(~scope :track 0)             ; band: live audio scope (evaluates to nil)
(~lane ...)                   ; band: lane mini-sequencer under a (lane ...) literal
```

All forms share one behavior: evaluate to a plain value (or nothing), register
a span-anchored widget as an eval side effect.

### State model: the widget holds no state; binding target depends on site

Existing widgets (`vslider` in a `ui.lisp`) are dumb: the author passes a
state binding. `~` forms differ — they hold no state either, but they
**auto-bind**, and what they bind to depends on where they sit:

- **Process inlet call site** (`:limit (~slider 12)`): the widget is a
  view+controller of the **inlet's pattern-scoped setting**. Dragging sends
  the value through the inlet via the existing handle write-through
  (identical to `(climb :limit 6)`). Scene-specific values come for free
  from the process model ("identity track-level, settings pattern-level" —
  already locked in the Cirklon spec), not from the widget: the slider is
  reactively bound to the setting (same reactive-field machinery as
  `bind-graph`) and re-renders on scene switch to show the current scene's
  value.
- **Bare literal site** (`(def cutoff (~slider 0.4 ...))`): no scene-scoped
  backing store exists — **the state is the text**. One value,
  scene-agnostic; dragging rewrites the literal and re-evals. Wanting
  scene-scoped values here is exactly the cue to promote to a process inlet.

What the text literal means at an inlet site (one literal, per-scene
settings): the literal is what buffer eval writes into the *current*
pattern's chain (per the Cirklon locked decision — re-eval never clobbers
other scenes' settings). Dragging write-throughs to the current scene and
rewrites the literal on release. After sculpting scene A and switching to
scene B, the slider shows B's value, which may differ from the literal; the
widget shows a "live ≠ text" badge/dim in that case, sharing the visual
language of the stale-anchor state.

- **Metadata fills omitted keywords at inlet call sites.** Inside a
  process-instance call, `(~slider 12)` pulls `:min`/`:max` (and step/kind)
  from the inlet's declared `:in` metadata (already retained since Cirklon
  Phase 0) — the `:in` declaration's triple duty (behavior input, UI surface,
  preset schema) becomes quadruple. Explicit keywords always override.
  Outside a metadata context, a range-needing widget with no `:min`/`:max` is
  an authoring error.

### Authoring walkthrough: minimal code per site

Three sites where "turn this parameter into a slider" happens, forming a
promotion path (hardcode → knob → parameter):

1. **Any bare literal — the floor.** `~` forms are not process-specific; they
   work on any literal in any evaluated buffer:

   ```lisp
   (def cutoff (~slider 0.4 :min 0 :max 1))   ; drag rewrites + re-evals
   ```

2. **Instance call site — explicit widget, range for free.** Because inlet
   metadata already exists, this beats Strudel's `slider(200, 0, 2000)` (which
   restates the range at every call site):

   ```lisp
   (transpose-climb :limit (~slider 12)              ; range 1..24 from :in decl
                    :delta (lane 0 1 0 0 1 0 0 0))
   ```

   Dragging writes through the existing handle path (`(climb :limit ...)`)
   live; the literal rewrites on release.

3. **Inside a `:run` body — tune a def's constant by ear.** Registration
   happens at def-compile time (span annotation runs at source conversion, not
   per-fire), so the runtime form is just the literal; dragging rewrites the
   text and hot-reloads the def:

   ```lisp
   :run (do
     (set! acc (clip (+ acc delta) 0 (~slider 24 :min 0 :max 48)))
     (target-add! acc))
   ```

   A body slider is a *proto-inlet*: when it proves to be a real parameter, it
   graduates to a declared `:in` entry — gaining lanes, p-locks, presets, and
   the metadata-inferred call-site form. A later "extract inlet" editor action
   could automate the promotion.
- Registration rides the existing span-annotation machinery: `~` forms are
  widget-producing expressions whose nodes carry the `__source-*` props plus
  an `:inline-anchor` marker; frame assembly pulls flagged nodes out of the
  normal widget-tree layout and into bands/margins. Reuses the committed
  snapshot + reactive machinery — no parallel widget pipeline.
- `~` forms in a file evaluated without an editor (headless/scripts) are
  transparent: value in, value out, no registration. Code with widgets stays
  runnable everywhere.

### Source of truth: write-back to text (locked)

Dragging a value widget **rewrites the literal in the buffer text** at the
anchored span (e.g. `12` → `14` inside `(~slider 12)`).

- Code stays the single truth: survives re-eval (buffer re-evaluation stays
  idempotent, matching the `processes` attachment philosophy), git-diffable,
  undo-able through the normal edit path.
- The rewrite *is* a buffer edit: it flows through the same anchor-adjustment
  seam (the widget's own span updates), marks the buffer dirty, and
  participates in undo history. Cursor position must be preserved across
  write-back (adjust like any other anchor).
- Live behavior while dragging: write-through to the runtime via the existing
  handle path (`(climb :limit 6)`-style follow-up) for immediate audio
  response, with the text rewrite either continuous or on drag-release —
  decide by feel in Phase 3; text-on-release + runtime-continuous is the
  likely answer (avoids undo-history spam).
- This is the same "sculpt with the knob, the code you end up with is the
  preset" property that makes this a prototyping surface rather than a
  performance overlay.

### The process tie-in (why this waits for the Cirklon work)

- **Inline slider on an inlet argument** = the existing handle knob-tweak
  write-through. No new runtime path.
- **Lane band under a `(lane ...)` literal** = the Cirklon spec's promised
  accumulator-curve preview (pure-fold processes), rendered directly under the
  literal that drives it. This is the killer app of the whole feature.
- **Binding chips**: a port declaration in code
  (`(cutoff (param-tag :cutoff))`) renders an inline margin chip showing
  bound/unbound/stale — same badge semantics as the Phase 3B slot UI — and
  clicking it arms the *same* wrapper arm-mode from `MACRO_MAPPING_SPEC.md`.
  One mapping infrastructure, two surfaces (slot UI for UI-people, inline chip
  for code-people). Do not build a code-side mapping mode.

## Implementation Plan

### Phase 1 — Display-row map + anchor table (the load-bearing slice)

1. Per-buffer anchor table with marker-semantics adjustment; funnel edit entry
   points (`editor/commands.rs`, `Buffer` methods) through one edit seam.
2. Display-row map in frame assembly (`ui/frame.rs`): bands reserve rows,
   line-number gutter stays buffer-line based, cursor/scroll/hit-test converted.
3. Prove it with a hardcoded test band (no `~` forms yet): a widget pinned
   after line N of a scratch buffer renders, scrolls, survives edits above it,
   and receives mouse interaction. Deterministic frame tests for the map
   (cursor round-trip, hit-test round-trip, band at viewport edges).

### Phase 2 — `~` registration + margin value widgets

1. `~slider` / `~knob` / `~toggle` forms: evaluate-to-value + registration via
   the existing span-annotation props; `:inline-anchor` flagged nodes pulled
   from normal layout into margins.
2. Headless transparency (no editor ⇒ pure passthrough).
3. Stale-anchor dimming; re-eval refresh.

### Phase 3 — Write-back + live write-through

1. Drag → runtime write-through (handle path) for immediate audio.
2. Drag-release → literal rewrite at the anchor span, through the normal edit
   path (undo, dirty, cursor preservation). Revisit continuous-vs-release.
3. Omitted-keyword metadata inference from process inlet declarations.

### Phase 4 — Bands with content

1. `~scope` band (reuse `live_audio`/`waveform` renderers).
2. `~lane` band: lane mini-sequencer under `(lane ...)` literals; wire the
   pure-fold accumulator curve preview when the Cirklon sugar tier provides it.

### Phase 5 — Binding chips + token highlights (later)

1. Inline port-binding chips sharing the Phase 3B arm-mapping seam.
2. Strudel-style active-token highlight decorations riding the same anchor
   table (likely via `Buffer.text_styles`), fed by play-position events.

## Locked Decisions

- Display-row map first; margin placement is a degenerate band, not a second
  system.
- Line numbers always render from buffer lines (bands own no line number).
- Anchor table with marker semantics; stale anchors dim, never vanish;
  re-eval refreshes.
- Write-back to text is the source of truth for value widgets; runtime
  write-through is a live preview of the pending text value.
- `~` forms are transparent (value passthrough) outside an editor context.
- `~` is the "inline" prefix on explicit widget names (`~slider`, `~knob`,
  `~scope`…); no bare inferring `(~ v)` form. Non-value arguments are
  keywords (`:min`/`:max`), never positional. Omitted keywords fill from
  inlet `:in` metadata at process call sites. `~` forms work on any literal
  in any evaluated buffer (not process-specific).
- Body-site `~` registers at def-compile time, never per-fire; a body slider
  is a proto-inlet that graduates to a declared `:in` entry.
- Binding chips reuse the `MACRO_MAPPING_SPEC.md` arm-mode; no code-only
  mapping path.
- Reuse the existing widget tree / committed-snapshot / reactive machinery for
  inline widgets — no parallel widget pipeline.
- `~` widgets hold no state. At inlet sites they reactively bind to the
  inlet's pattern-scoped setting (scene-aware; drags go through the handle
  write-through); at bare-literal sites the text is the state
  (scene-agnostic). "Live ≠ text" gets a badge, not silent divergence.

## Open Questions

- Continuous vs. on-release text write-back while dragging (undo-history
  granularity); likely release-only with continuous runtime preview.
- Exact `~` vocabulary: is `~scope`/`~lane` right, or does a general
  `(~widget <type> ...)` escape hatch earn its keep alongside the sugar?
- Band height policy: fixed per widget type vs. author-specified
  `:height` — and whether bands can be collapsed/folded like code.
- How `~` forms interact with buffers evaluated as overlays/modules
  (`snapshot_file_backed_sources`) — anchors belong to the buffer the span
  came from; verify `__source-buffer-id` is correct for file-backed overlay
  eval.
- Whether token-highlight decorations (Phase 5) need their own cheaper anchor
  representation (many short-lived anchors per beat) or ride `text_styles`
  with play-position invalidation.
