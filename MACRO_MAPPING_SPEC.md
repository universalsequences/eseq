# Macro / Parameter-Mapping Spec & Implementation Plan

Status: draft / not yet implemented
Scope: Phases 1–4 (engine override, write path + commands, mapping-mode UI, macro
panel). Phase 5 (higher-order / neural-sequencer triggering of macro indices) is
explicitly **out of scope** here but the design preserves the seam for it.

---

## 1. Goal

Let the user, from inside the sequencer UI, create **macros**: project-global
controls that drive an arbitrary set of instrument/effect parameters at once
("push effects"). A macro:

- holds one live value `0..1`,
- maps to N target parameters, each with its own min/max range (and curve),
- is driven by a **continuous knob** and/or a **momentary button** (press =
  engage toward 1, release = pop back to 0),
- applies **non-destructively**: the underlying base/UI value of each target is
  never mutated; releasing the macro instantly restores the base.

Mapping is performed with an Ableton-style "arm" interaction: enter mapping mode
for a macro, every mappable knob highlights in a distinct color, click a knob to
add it to the macro.

### Design decisions (locked)

| Decision | Choice |
|---|---|
| Override model | Engine **live-override layer** (sample-accurate, composes with p-locks/automation, true instant pop-back). Not host-side snapshot/restore. |
| Macro scope | **Project-global**. A macro can map to any instrument or effect param anywhere. |
| Triggers | **Continuous knob** + **momentary button**. Sequenceable macro value is deferred (Phase 5). |
| Range anchoring | **Absolute** device-unit min/max captured at map time. While engaged, the param is fully macro-controlled (UI edits to base are masked until release). |

---

## 2. Background: how params flow today (verified seams)

Understanding the existing param path is what makes the override layer cheap.

### 2.1 Live param edits

A UI knob edit routes:

```
param-controls.lisp  fx-set-effect-value / fx-set-instrument-value
  → host-command "set-effect-param" / "set-instrument-param"
    → src/bin/metal_seq/main.rs dispatch (set-effect-param @ ~6131,
                                           set-instrument-param @ ~5968)
      → ui::apply_command(AppCommand::SetEffectParam{..})   (src/ui/command.rs)
```

`AppCommand::SetEffectParam` (`src/ui/command.rs:1283`) does **two** things:

```rust
slot.defaults.set(param_idx, value);          // (A) the live/base value store
app.send_slot_param(track, slot_idx, param_idx, value);  // (B) push to DSP node
```

`AppCommand::SetInstrumentParam` (`src/ui/command.rs:1338`) is the analog with
`slot.defaults.set` + `app.send_instrument_param`.

**Key fact:** `slot.defaults` is the canonical *live base value* (it is what the
UI knob reflects and what is persisted). `send_*_param` is the only thing the DSP
node actually hears. The override layer therefore lives **between (A) and (B)**:
the macro must change what (B) sends without touching (A).

### 2.2 Step-trigger params (p-locks) — for contrast, NOT the macro path

`scheduler.rs:resolved_slot_param_value` / `resolve_effect_params` /
`resolve_instrument_params` resolve params at **note trigger** time, layering a
per-step p-lock over `slot.defaults`. These are step-scoped and identity-guarded
by `ParamNodeId` (`slot_param_identity`, `plock_identity_matches`). Macros are
*live*, not step-tied, so they do **not** go through these functions — but they
reuse the same `ParamNodeId` concept for stable target identity, and the override
must coexist with p-locks (see §4.4 precedence).

### 2.3 Modulation mapping mode — the UI template

`metal-seq-fx/param-controls.lisp` already implements an arm-and-highlight UI:

- Global flags `instrument-mods-open` / `effect-mods-open` (in
  `metal-seq-fx/state.lisp`).
- `param-mod-wrapper` (`param-controls.lisp:182`) and
  `instrument-param-mod-wrapper` (`:379`) wrap each `:modulatable` param in a box
  with blue highlight `(rgba 0.18 0.48 0.95 0.24)`, swap the knob's min/max to the
  mod target's depth range, and bind `on-double-click` to toggle the connection.
- `effect-modulation.lisp` builds the source selector / editor panel.

The macro mapping UI is a **recolored fork** of this wrapper with a different
click action (map-to-macro) and a different highlight color.

---

## 3. Phase 1 — Engine live-override layer

The single new primitive. Everything else is plumbing.

### 3.1 Concept

Introduce a **MacroEngine** that owns the macro definitions and an active
override table. The effective value pushed to any DSP node becomes:

```
effective(param) = macro_override(param).unwrap_or(slot.defaults.get(param))
```

`slot.defaults` is never written by the macro. When a macro disengages (value
returns to identity / button released, and no other macro overrides the param),
the override entry is removed and the base value is re-sent — instant pop-back.

### 3.2 Data structures

New module `src/macros.rs` (audio/UI-thread state; not RT-audio — see §3.5):

```rust
pub struct MacroEngine {
    macros: Vec<Macro>,
    /// Active overrides keyed by resolved target identity. Value is the
    /// macro-driven value currently forced onto the DSP node.
    overrides: HashMap<MacroParamKey, f32>,
}

pub struct Macro {
    pub id: MacroId,            // stable, never reused (see §6 persistence)
    pub name: String,
    pub value: f32,             // 0..1 live position
    pub mappings: Vec<MacroMapping>,
}

pub struct MacroMapping {
    pub target: MacroTarget,
    pub range_min: f32,         // device units, captured at map time
    pub range_max: f32,
    pub curve: MacroCurve,      // Linear (default) | Exp | Log
}

pub enum MacroTarget {
    Instrument { track: usize, param_idx: usize },
    Effect {
        track: usize, slot_idx: usize, param_idx: usize,
        // identity guard so a mapping survives node rebuilds, same idea as plocks
        logical_id: ParamNodeId,
    },
    // Bus effects: add `BusEffect { bus: u64, slot_idx, param_idx, logical_id }`
    // when bus targeting lands (mirror of fx :bus-fx branch).
}

/// Stable hash key for the overrides table — derived from target, robust to
/// transient slot reordering only via logical_id (effects).
pub enum MacroParamKey { Instrument(usize, usize), Effect(/*logical*/ u64, u64) }

pub type MacroId = u32;
```

### 3.3 The effective-value send path

Add to `App` (where `send_slot_param` / `send_instrument_param` live):

```rust
/// Push a param to the DSP node honoring the macro override layer.
/// ALL param writes (UI edits AND macro recompute) go through here.
fn send_effective_slot_param(&mut self, track, slot_idx, param_idx) {
    let base = self.state.pattern.effect_chains[track][slot_idx]
                   .defaults.get(param_idx);
    let key = self.macro_key_for_effect(track, slot_idx, param_idx);
    let v = self.macro_engine.overrides.get(&key).copied().unwrap_or(base);
    self.send_slot_param(track, slot_idx, param_idx, v);
}
// + send_effective_instrument_param analog.
```

Then **rewire the two apply_command arms** so UI edits update the base but emit
the effective value:

```rust
AppCommand::SetEffectParam { .. } => {
    slot.defaults.set(param_idx, value);                 // base unchanged-by-macro
    app.send_effective_slot_param(track, slot_idx, param_idx);  // was send_slot_param
    sync_effect_mod_active_default(..);
}
```

This guarantees that turning a knob while a macro is engaged updates the *stored
base* (so it pops back to the new value on release) but does **not** override the
live macro value — the override still wins until released.

### 3.4 Macro recompute

```rust
impl MacroEngine {
    /// Called whenever a macro's value changes (knob drag, button press/release).
    /// Returns the set of (target) params whose effective value must be re-sent.
    fn set_value(&mut self, id: MacroId, value: f32) -> Vec<MacroTarget> {
        let m = self.macro_mut(id);
        m.value = value;
        let mut touched = vec![];
        for map in &m.mappings {
            let key = map.target.key();
            if value_is_identity(value) /* e.g. == 0.0 within eps */ {
                // disengage: drop override unless another macro still owns it
                self.maybe_release(key);
            } else {
                let v = lerp_curved(map.range_min, map.range_max, value, map.curve);
                self.overrides.insert(key, v);  // last-writer-wins on contested params
            }
            touched.push(map.target.clone());
        }
        touched
    }
}
```

The App layer takes `touched` and calls `send_effective_*` for each, so the DSP
nodes update. On button release the App calls `set_value(id, 0.0)` which releases
overrides and re-sends base values → pop-back.

### 3.5 Threading / RT-safety

`send_slot_param` already crosses to the audio graph; macro recompute happens on
the UI/command thread (same thread as `apply_command`), so the override table is
**not** touched from the RT audio callback. No new locks. If, in Phase 5, a graph
node needs to drive a macro, the value will be marshalled onto the command thread
(out of scope here, but the table stays single-threaded).

### 3.6 Phase 1 deliverables

- `src/macros.rs` with `MacroEngine`, `Macro`, `MacroMapping`, `MacroTarget`,
  `MacroParamKey`, `lerp_curved`.
- `App.macro_engine: MacroEngine` field + `send_effective_slot_param` /
  `send_effective_instrument_param`.
- Rewire `SetEffectParam` / `SetInstrumentParam` apply arms to use the effective
  send.
- Unit tests: override masks base; base edit while engaged pops back to new base;
  release with no other owner restores; two macros on one param = last-writer-wins
  then correct release ordering; identity epsilon.

---

## 4. Phase 2 — Macro write path + host commands

### 4.1 New `AppCommand` variants (`src/ui/command.rs`)

```rust
MacroCreate { name: String },
MacroDelete { id: MacroId },
MacroRename { id: MacroId, name: String },
MacroSetValue { id: MacroId, value: f32 },         // knob + button both use this
MacroMapParam { id: MacroId, target: MacroTarget }, // capture current value as anchor
MacroSetRange { id: MacroId, mapping_idx: usize, min: f32, max: f32 },
MacroSetCurve { id: MacroId, mapping_idx: usize, curve: MacroCurve },
MacroUnmap { id: MacroId, mapping_idx: usize },
```

`MacroMapParam` resolves the target's *current* value/min/max from the effect
descriptor (same descriptor lookup `set-effect-param` already does at
`main.rs:6155`, `app.graph.effect_descriptors[track][slot_idx].params[param_idx]`)
and seeds `range_min`/`range_max` from the descriptor's min/max (user narrows
later). It also records the `logical_id` via the existing
`slot_param_identity(node_id, modulator_node_id, raw_idx)` so the mapping is
rebuild-safe.

`MacroSetValue` calls `MacroEngine::set_value` and fans `send_effective_*` over
the returned targets.

### 4.2 Host-command dispatch (`src/bin/metal_seq/main.rs`)

Register alongside `set-effect-param` (~6131) / `set-instrument-param` (~5968):

| Command | Payload | → AppCommand |
|---|---|---|
| `macro-create` | `{name}` | `MacroCreate` |
| `macro-delete` | `{id}` | `MacroDelete` |
| `macro-rename` | `{id, name}` | `MacroRename` |
| `macro-set-value` | `{id, value}` | `MacroSetValue` |
| `macro-map-param` | `{id, kind, track, slot-idx?, param-idx, bus?}` | `MacroMapParam` |
| `macro-set-range` | `{id, mapping-idx, min, max}` | `MacroSetRange` |
| `macro-set-curve` | `{id, mapping-idx, curve}` | `MacroSetCurve` |
| `macro-unmap` | `{id, mapping-idx}` | `MacroUnmap` |

Follow the existing payload-extraction idiom (`map.get("…").and_then(borrow →
Number/Str)`). The `macro-map-param` payload mirrors the target descriptor
`fx-set-effect-value` already assembles in `param-controls.lisp:21` (bus-fx /
midi-fx / instrument branches), so the Lisp side reuses that shape.

### 4.3 Reading macro state back into the UI

The UI needs to render macro list, mappings, ranges, and live values. Follow the
project's existing state-readback mechanism (the same channel `seq-…` / state
snapshot used for effect params — see `src/bin/metal_seq/state_values.rs`). Expose:

- `macros` → list of `{id, name, value, mappings: [{target-label, min, max, curve, current}]}`
- `macro-mapping-open` reflection is UI-local (Lisp `defstate`), not engine state.

### 4.4 Override vs. p-lock precedence (must specify)

Both can target the same param. Rule: **a step p-lock wins over a macro override
at trigger time** (it is the more specific, explicitly-sequenced intent), while
the macro override is the live baseline between/around triggers. Because p-locks
resolve in `resolved_slot_param_value` at trigger and macros write live via
`send_effective_*`, the natural behavior is: p-lock briefly forces its value at
the triggered step, macro override governs otherwise. Document and test this; if a
conflict feels wrong in practice, gate macro override application on
"param has no active p-lock this step."

### 4.5 Phase 2 deliverables

- AppCommand variants + apply bodies.
- Host-command dispatch arms.
- State readback for macros.
- Tests: map captures current value + identity; range edit; unmap releases
  override; delete releases all its overrides and renumbers nothing (IDs stable).

---

## 5. Phase 3 — Mapping-mode UI (highlight fork)

### 5.1 New UI state (`metal-seq-fx/state.lisp`)

```lisp
(defstate macro-mapping-open false)   ;; arm flag — global, like effect-mods-open
(defstate macro-mapping-selected -1)  ;; MacroId currently being mapped, -1 none
```

### 5.2 Wrapper fork (`metal-seq-fx/param-controls.lisp`)

Extend `param-mod-wrapper` (:182) and `instrument-param-mod-wrapper` (:379) with a
macro branch that takes precedence checks alongside the existing mods-open branch:

```lisp
(def param-macro-mapping-active? ()
  (and macro-mapping-open (>= macro-mapping-selected 0)))

(def param-macro-bg (p)
  (if (and (param-macro-mapping-active?) (get p :modulatable))
    (rgba 0.18 0.85 0.42 0.26)   ;; GREEN — distinct from modulation blue
    :transparent))
```

When `macro-mapping-open`:
- highlight every `:modulatable` param green (across *all* open device panels —
  this is what enables cross-device mapping in one gesture),
- `on-click` → `host-command "macro-map-param" {...target...}` using the same
  target-descriptor assembly as `fx-set-effect-value` / `fx-set-instrument-value`,
- already-mapped params for the selected macro get a brighter/filled border so the
  user sees the current set,
- double-click an already-mapped param → `macro-unmap`.

Precedence: if both `effect-mods-open` and `macro-mapping-open` were somehow on,
macro mapping mode wins (or make them mutually exclusive — recommend mutually
exclusive: opening one closes the other).

### 5.3 Range editing while armed

Reuse the modulation trick: while `macro-mapping-open`, a mapped knob's `:min`/
`:max` read from the mapping's `range_min`/`range_max` and dragging the knob edits
the **range endpoint** (which endpoint = a small toggle, or drag = max / shift-drag
= min), not the base value. This is the direct analog of how mods-open swaps the
knob to edit depth. Mirror `param-control-min` / `param-control-max`
(`param-controls.lisp:109`/`115`).

### 5.4 Phase 3 deliverables

- 2 new defstates.
- Macro branch in both param wrappers (green highlight, map/unmap clicks).
- Mapped-state visual (border) + range-edit-on-drag while armed.
- Mutual exclusion with modulation mode.

---

## 6. Phase 4 — Macro panel + drive controls

A new panel modeled on `metal-seq-fx/modulator-panel.lisp` /
`effect-modulation.lisp` (`effect-mod-control-panel`).

### 6.1 Layout

```
┌ Macros ───────────────────────────────────────────────┐
│ [Macro A*] [Macro B] [+ new]      (selector row)       │
│                                                        │
│  ( ) Macro A          [arm-map]  [×]                   │
│   knob: ◯ 0.42        button: ▢ (momentary)            │
│                                                        │
│  mappings:                                             │
│   • Reverb · size     [min .. max]  curve▾   [unmap]   │
│   • Delay · feedback  [min .. max]  curve▾   [unmap]   │
│   • Filter · cutoff   [min .. max]  curve▾   [unmap]   │
└────────────────────────────────────────────────────────┘
```

### 6.2 Controls

- **Selector row**: one button per macro (active = highlighted like
  `effect-mod-selector-row`'s selected state), `+ new` → `macro-create`.
- **Macro knob**: continuous `0..1`, `on-change → macro-set-value {id, v}`.
  Reuse `knob-number` as in `modulator-knob`.
- **Momentary button**: `on-press → macro-set-value {id, 1.0}` (or a configurable
  engage target), `on-release → macro-set-value {id, 0.0}`. Requires press/release
  events — confirm the button widget exposes both (mod source ON/OFF buttons use
  `on-click`; the momentary needs `on-press`/`on-release` — if absent, add to the
  widget, small).
- **Arm-map button**: toggles `macro-mapping-open` + sets `macro-mapping-selected`
  to this macro's id. Visually "armed" while active (Ableton-style).
- **Mapping rows**: target label, `number-picker` min + max → `macro-set-range`,
  curve dropdown → `macro-set-curve`, unmap button → `macro-unmap`. Show the
  param's live resolved value as a faint readout.

### 6.3 Where it mounts

Project-global, so it is **not** inside a per-device panel. Mount it as its own
strip/tab in the FX area (sibling to the effect/instrument panels — see how
panels are assembled in `metal-seq-fx.lisp` / `panel-bodies.lisp`). A toggle in
the transport or FX header opens it.

### 6.4 Phase 4 deliverables

- `metal-seq-fx/macro-panel.lisp` (new file; add to the load list).
- Macro selector + knob + momentary button + arm-map.
- Mapping rows with range/curve/unmap.
- Mount point + open/close toggle.
- `on-press`/`on-release` on the button widget if not already present.

---

## 7. Persistence (`src/project.rs`)

Add a project-global macro list, alongside `ProjectModConnection` (`:295`) and
`ProjectPattern` (`:229`):

```rust
pub struct ProjectMacro {
    pub id: u32,
    pub name: String,
    pub value: f32,
    pub mappings: Vec<ProjectMacroMapping>,
}
pub struct ProjectMacroMapping {
    pub target: ProjectMacroTarget, // serde-tagged like ProjectModDestination
    pub range_min: f32,
    pub range_max: f32,
    #[serde(default)] pub curve: ProjectMacroCurve,
}
```

- Add `#[serde(default)] pub macros: Vec<ProjectMacro>` to `ProjectFile` so old
  projects load (empty list).
- **IDs are stable and never reused** even across delete (monotonic counter
  persisted, or tombstone). This is the seam Phase 5 depends on: a future
  neural/graph node referencing "macro 3" must stay valid. Do not renumber on
  delete.
- Effect targets persist `logical_id` so re-pointing survives effect reloads; on
  load, re-resolve against the live descriptor and drop/flag mappings whose target
  no longer exists (mirror `plock_identity_matches` staleness handling).

---

## 8. Testing strategy

Rust (engine, the high-risk surface):
- override masks base; base edit while engaged → correct pop-back target;
  release restores; identity epsilon; curve math; last-writer-wins for two macros
  on one param + ordered release; map captures current value & logical_id;
  stale-identity mapping dropped on reload; p-lock vs macro precedence (§4.4).

Lisp/UI (layout + interaction, use the existing layout-test harness):
- mapping-mode green highlight appears on `:modulatable` params only;
  click in armed mode emits `macro-map-param` with correct descriptor;
  range-edit-on-drag changes range not base; mutual exclusion with mods mode;
  panel renders with `each` over macros (per `[[lisp-ui-each-vs-map]]` — use
  `each`, never `map`).

Manual / `/run`:
- Map cutoff+feedback+size across 3 effects to one macro; sweep knob → all move;
  release momentary button → instant pop-back; reload project → mappings intact.

---

## 9. Risks / open questions

1. **§4.4 precedence** (macro vs p-lock vs per-step automation on one param) is
   the subtlest behavior — get the rule right and test it explicitly.
2. **Masking base edits while engaged**: decision is "macro fully owns the param
   while engaged." Confirm this matches the feel you want vs. relative offset.
   Absolute is simpler and chosen; relative-offset is a possible future curve mode.
3. **Momentary button events**: verify `on-press`/`on-release` exist on the button
   widget; if not, add them (small, but a real task).
4. **Bus-effect targets**: spec covers track instrument + track effects first;
   `MacroTarget::BusEffect` is a straight mirror of the `:bus-fx` branch, add in a
   follow-up slice if bus targeting is wanted at v1.
5. **ID stability** must be honored from day one (Phase 5 depends on it) even
   though Phase 5 isn't built here.

---

## 10. Implementation order (critical path)

1. **Phase 1** — engine override layer + effective-send rewire + Rust tests.
   (Highest risk/value; everything else is plumbing once this is correct.)
2. **Phase 2** — commands + dispatch + state readback + tests.
3. **Phase 4 controls minimal** — macro create/select + knob + arm-map button,
   enough to exercise Phase 1/2 end-to-end via `/run`.
4. **Phase 3** — mapping-mode highlight fork + click-to-map + range-edit.
5. **Phase 4 full** — mapping rows, momentary button, curve, mount/toggle.
6. **Phase 7 persistence** can land with Phase 2 (needed for reload tests).

Phases 1–2 + a minimal Phase 4 give a testable vertical slice; 3 + full 4 complete
the intended UX.
