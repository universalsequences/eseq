# Macro / Parameter-Mapping Spec & Implementation Plan

Status: draft / not yet implemented (rev 2 — updated after the def-process layer
landed; seams re-verified 2026-07-11)
Scope: Phases 1–4 (engine override, write path + commands, mapping-mode UI,
script-authored macro controls) + Phase 6 (scene macros / "push scenes"). Phase 5 (process- or
sequencer-driven macro values) is explicitly **out of scope** here but the design
preserves the seam for it — and that seam got much closer with the process layer
(see §2.4, §9.1).

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

**Scene macros (Phase 6)** extend this: a macro whose target set is derived
automatically by *diffing another scene* against the live state. Press the
button → the params that differ get overrides pointing at the target scene's
values; the knob interpolates between "where I was" (0) and "fully scene N" (1);
release → pop back. Optionally the same gesture also *steals the target scene's
patterns* (quantized), independently toggleable from the param morph. See §8.

### Design decisions (locked)

| Decision | Choice |
|---|---|
| Override model | Engine **live-override layer** (sample-accurate, composes with p-locks/automation, true instant pop-back). Not host-side snapshot/restore. |
| Macro scope | **Project-global**. A macro can map to any instrument or effect param anywhere. |
| Script identity | Each macro may have an immutable, project-unique **stable key**. Script-authored surfaces use the key; numeric `MacroId` remains the engine/persistence identity. Re-evaluating a script must ensure/reuse, never duplicate, its keyed macros. |
| Triggers | **Continuous knob** + **momentary button**. A continuous mapped macro remains engaged at `0` (its mapped minimum); momentary release is an explicit operation. Sequenceable macro value is deferred (Phase 5). |
| Range anchoring | **Absolute** device-unit min/max captured at map time. While engaged, the param is fully macro-controlled (UI edits to base are masked until release). |
| Mapping ownership | **Exclusive per parameter.** A parameter may belong to at most one macro. Mapped device controls are read-only and carry a green ownership dot; ranges are edited in the mapping table. |
| Target type | **Reuse `crate::process::ParamTarget`** (new in rev 2) — do not invent a parallel `MacroTarget` enum. See §2.4/§3.2. |
| Scene-macro anchoring | **Live reference, diff at press time** (not a snapshot captured at map time). Editing scene 4 changes what the push does. See §8.2. |

---

## 2. Background: how params flow today (seams re-verified 2026-07-11)

Understanding the existing param path is what makes the override layer cheap.
Line numbers below are current as of branch `codex/band-mode`; treat them as
anchors, re-grep before coding.

### 2.1 Live param edits

A UI knob edit routes:

```
param-controls.lisp  fx-set-effect-value / fx-set-instrument-value
  → host-command "set-effect-param" / "set-instrument-param"
    → src/bin/metal_seq/main.rs dispatch ("set-instrument-param" @ ~7929,
                                           "set-effect-param" @ ~8360)
      → ui::apply_command(AppCommand::SetEffectParam{..})   (src/ui/command.rs)
```

The `AppCommand::SetEffectParam` apply arm (`src/ui/command.rs:1845`) does
**two** things:

```rust
slot.defaults.set(param_idx, value);          // (A) the live/base value store
app.send_slot_param(track, slot_idx, param_idx, value);  // (B) push to DSP node
```

The `AppCommand::SetInstrumentParam` apply arm (`src/ui/command.rs:1900`) is the
analog with `slot.defaults.set` + `app.send_instrument_param`.

The send functions themselves live in `src/ui/effect_params.rs:484`
(`send_slot_param`) and `src/ui/synth.rs:932` (`send_instrument_param`).

**Key fact:** `slot.defaults` is the canonical *live base value* (it is what the
UI knob reflects and what is persisted). `send_*_param` is the only thing the DSP
node actually hears. The override layer therefore lives **between (A) and (B)**:
the macro must change what (B) sends without touching (A).

### 2.2 Step-trigger params (p-locks) — for contrast, NOT the macro path

`scheduler.rs` resolves params at **note trigger** time, layering per-step
p-locks over `slot.defaults`. Identity-guarding uses `slot_param_identity`
(`scheduler.rs:428`) / `plock_identity_matches` (`scheduler.rs:451`) with
`ParamNodeId`, so plocks survive node rebuilds. Macros are *live*, not
step-tied, so they do **not** go through these functions — but they reuse the
same `ParamNodeId` identity concept, and the override must coexist with p-locks
(see §4.4 precedence).

### 2.3 Modulation mapping mode — the UI template

`metal-seq-fx/param-controls.lisp` already implements an arm-and-highlight UI:

- Global flags `instrument-mods-open` / `effect-mods-open` (in
  `metal-seq-fx/state.lisp`).
- `param-mod-wrapper` (`param-controls.lisp:396`) and
  `instrument-param-mod-wrapper` (`:605`) wrap each `:modulatable` param in a box
  with blue highlight `(rgba 0.18 0.48 0.95 0.24)`, swap the knob's min/max to the
  mod target's depth range, and bind `on-double-click` to toggle the connection.
- Range accessors: `param-control-min`/`-max` (`:323`/`:329`),
  `instrument-param-control-min`/`-max` (`:548`/`:554`).
- `effect-modulation.lisp` builds the source selector / editor panel.

The macro mapping UI is a **recolored fork** of this wrapper with a different
click action (map-to-macro) and a different highlight color.

### 2.4 The process layer (NEW since rev 1) — reuse its target machinery

The def-process work landed a first-class param-targeting system in
`src/process.rs` that overlaps heavily with what rev 1 of this spec was about to
invent. The macro engine must **reuse it, not duplicate it**:

- **`ParamTarget`** (`process.rs:331`) is the canonical "a knob somewhere" type:
  `InstrumentParam { param, param_id: Option<ParamNodeId> }`,
  `EffectParam { slot, effect, param, param_id }`, `MidiFxParam`,
  `RackSlotParam` / `RackSlotInstrumentParam`, `ProcessInlet`, `StepParam`.
  It is already `Serialize`/`Deserialize` (project persistence for free) and
  already carries the `ParamNodeId` identity guard.
- **Rebuild-safe re-resolution already exists**:
  `refresh_track_process_chain_binding_param_ids` (`process.rs:521`) and the
  per-slot variant (`process.rs:561`, called from
  `TrackPatternData::refresh_process_effect_binding_param_ids_for_slot`,
  `sequencer/state.rs:476`) re-stamp `param_id`s when effect nodes rebuild.
  The macro engine hooks the same refresh points for its mappings.
- **Process target writes are a trigger-time layer, not a live layer.**
  `apply_process_target_writes` (`scheduler.rs:3370`) applies
  `ProcessTargetWrite { op: Set|Add, .. }` values transiently at step-trigger
  time inside the scheduler (test:
  `scheduler_process_target_writes_are_transient_step_param_writes`,
  `scheduler.rs:10237`). They never touch `slot.defaults` and never use the
  live send path. This means macros (live sends) and process writes
  (trigger-time resolution) occupy **different layers** and compose rather than
  conflict — see the updated precedence model in §4.4.
- **Process chains are per-scene state** (`TrackPatternData.process_chain`,
  `Scene.project_process_chain`), while macros are **project-global** and live
  outside scene snapshots. A scene switch changes process chains but must not
  disturb engaged macros (§3.6).

What macros add that the process layer doesn't have: a *live* (non-step,
non-trigger) override on the send path with instant pop-back, and a
one-gesture N-param mapping UI. What the process layer offers macros later:
sequencing/automation of the macro value itself (Phase 5 becomes "a process
whose outlet targets a macro" — see §9.1).

### 2.5 Scene switching (matters for §3.6 and Phase 6)

- A **Scene** (`sequencer/state.rs:897`) = per-track pattern cells (`PatternId`
  into `TrackPatternPool`s) + `bus_patterns` + scene-level mod/neural/graph
  routing + `project_process_chain`.
- Each track's cell resolves to a **`TrackPatternData`** (`state.rs:451`) which
  carries the *full per-scene param state*: `effect_slots` /
  `instrument_slot` (`EffectSlotSnapshot`s with per-scene defaults),
  `midi_fx_slots`, `track_params`, `process_chain`. Bus effects likewise keep
  per-scene `effect_defaults` (`BusPatternSnapshot`, `state.rs:211`). **This is
  why "scene 4 has crazy delays" works at all — and it is exactly the data the
  scene-macro diff reads (§8.2).**
- Switch path: host command `"switch-pattern"` (`main.rs:12531`) →
  `state.switch_pattern(..)` (profiled variant `state.rs:5880`) →
  **`push_all_restored_defaults`** (`src/ui/projects.rs:2587`), which re-sends
  every restored `slot.defaults` value to the DSP via `send_slot_param` /
  `push_*`. Per-track launches (`"launch-track-pattern"`, `main.rs:12380`) and
  `scene_silenced` handling ride the same machinery.

---

## 3. Phase 1 — Engine live-override layer

The single new primitive. Everything else is plumbing.

### 3.1 Concept

Introduce a **MacroEngine** that owns the macro definitions and an active
override table. The effective value pushed to any DSP node becomes:

```
effective(param) = macro_override(param).unwrap_or(slot.defaults.get(param))
```

`slot.defaults` is never written by the macro. A continuous mapped macro stays
engaged across its full `0..1` domain: `0` means `range_min`, not release. When
a momentary macro is explicitly released, the override entry is removed and
the base value is re-sent — instant pop-back.

### 3.2 Data structures

New module `src/macro_engine.rs` (UI/command-thread state; not RT-audio — see
§3.5). Note the name: `macros.rs` invites confusion with Rust macro modules.

```rust
use crate::process::ParamTarget;   // REUSED — see §2.4. No new target enum.

pub struct MacroEngine {
    macros: Vec<Macro>,
    next_id: MacroId,               // monotonic, never reused (§7)
    /// Active overrides keyed by resolved target identity. Value is the
    /// macro-driven value currently forced onto the DSP node.
    overrides: HashMap<MacroParamKey, f32>,
}

pub struct Macro {
    pub id: MacroId,                // stable, never reused (see §7 persistence)
    pub key: Option<String>,         // immutable script key; unique within project
    pub name: String,
    pub value: f32,                 // 0..1 live position
    pub mappings: Vec<MacroMapping>,
    pub kind: MacroKind,            // Mapped (Phases 1–4) | Scene (Phase 6)
}

pub enum MacroKind {
    Mapped,                          // hand-mapped param set
    Scene(SceneMacroConfig),         // §8 — mappings derived by scene diff
}

pub struct MacroMapping {
    /// Track context + the target. ParamTarget is track-relative (slot/param),
    /// so the mapping stores the track index alongside it, mirroring how
    /// TrackProcessSlot bindings are scoped to their owning track.
    pub track: usize,
    pub target: ParamTarget,        // param_id inside = rebuild identity guard
    pub range_min: f32,             // device units, captured at map time
    pub range_max: f32,
    pub curve: MacroCurve,          // Linear (default) | Exp | Log
}

/// Stable hash key for the overrides table, derived from (track, target).
/// For effect/instrument targets prefer the ParamNodeId when present so the
/// key survives slot reordering; fall back to (track, slot, param) indices.
pub struct MacroParamKey(/* see above */);

pub type MacroId = u32;
```

Deliberately **not** supported as macro targets in v1 even though `ParamTarget`
can express them: `StepParam` (trigger-time, belongs to processes/p-locks) and
`ProcessInlet` (write process inlets through the process command path instead;
revisit if a real use case appears). Validate and reject at map time.

### 3.3 The effective-value send path

Add to `App` (near `send_slot_param` / `send_instrument_param`):

```rust
/// Push a param to the DSP node honoring the macro override layer.
/// ALL live param writes (UI edits, macro recompute, scene-switch restore)
/// go through here.
fn send_effective_slot_param(&mut self, track, slot_idx, param_idx) {
    let base = self.state.pattern.effect_chains[track][slot_idx]
                   .defaults.get(param_idx);
    let key = self.macro_key_for_effect(track, slot_idx, param_idx);
    let v = self.macro_engine.overrides.get(&key).copied().unwrap_or(base);
    self.send_slot_param(track, slot_idx, param_idx, v);
}
// + send_effective_instrument_param analog.
```

Then **rewire the two apply_command arms** (`command.rs:1845` / `:1900`) so UI
edits update the base but emit the effective value:

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
    /// Called whenever a macro's value changes (knob drag or button press).
    /// Returns the set of (track, target) params whose effective value must be
    /// re-sent.
    fn set_value(&mut self, id: MacroId, value: f32) -> Vec<(usize, ParamTarget)> {
        let m = self.macro_mut(id);
        m.value = value;
        let mut touched = vec![];
        for map in &m.mappings {
            let key = MacroParamKey::from(map);
            let v = lerp_curved(map.range_min, map.range_max, value, map.curve);
            self.overrides.insert(key, v);  // mapping ownership is exclusive
            touched.push((map.track, map.target.clone()));
        }
        touched
    }

    /// Momentary controls release explicitly; value zero remains meaningful.
    fn release(&mut self, id: MacroId) -> Vec<(usize, ParamTarget)> { /* ... */ }
}
```

The App layer takes `touched` and calls `send_effective_*` for each, so the DSP
nodes update. Mapping a continuous macro immediately owns the parameter at its
current `0..1` position, including zero. On momentary button release the App
calls `release(id)`, which removes overrides and re-sends base values → pop-back.

### 3.5 Threading / RT-safety

`send_slot_param` already crosses to the audio graph; macro recompute happens on
the UI/command thread (same thread as `apply_command`), so the override table is
**not** touched from the RT audio callback *or the scheduler/lookahead thread*.
The command thread folds a copy into immutable scheduler snapshots, so neither
audio path locks the engine. Process target writes (§2.4) layer over those
macro-effective snapshot defaults. If,
in Phase 5, a process outlet drives a macro, the value is marshalled onto the
command thread (out of scope here, but the table stays single-threaded).

### 3.6 Scene switches while engaged (NEW)

`push_all_restored_defaults` (`projects.rs:2587`) re-sends raw
`slot.defaults` after a scene switch — under an engaged macro this would clobber
live overrides with base values. Two-part rule:

1. **Route the restore through the effective layer**: the per-param send inside
   `push_all_restored_defaults` becomes `send_effective_*` (or the macro engine
   re-asserts its overrides immediately after the restore pass — pick whichever
   is cheaper given the loop already has descriptor context; routing through
   `send_effective_*` is the simpler invariant: *no live param send bypasses the
   layer*).
2. **Re-validate mappings after the switch**: the new scene may have a different
   effect chain on a mapped track. Run the same staleness pass as project load
   (§7): mappings whose `ParamNodeId` no longer resolves are suspended (kept,
   flagged, override released) until a scene where they resolve again. This
   mirrors how process bindings are refreshed per-slot on rebuilds
   (`refresh_track_process_chain_effect_binding_param_ids_for_slot`).

Scene *macros* (Phase 6) have their own rule here — the morph is released on
manual scene switch (§8.5).

### 3.7 Phase 1 deliverables

- `src/macro_engine.rs` with `MacroEngine`, `Macro`, `MacroKind`,
  `MacroMapping` (over `process::ParamTarget`), `MacroParamKey`, `lerp_curved`.
- `App.macro_engine: MacroEngine` field + `send_effective_slot_param` /
  `send_effective_instrument_param`.
- Rewire `SetEffectParam` / `SetInstrumentParam` apply arms + the
  `push_all_restored_defaults` restore path to use the effective send.
- Unit tests: override masks base; base edit while engaged pops back to new base;
  explicit release restores; a second macro cannot claim an owned parameter;
  zero remains engaged at `range_min`; scene switch under engaged
  macro re-asserts overrides and suspends stale mappings.

---

## 4. Phase 2 — Macro write path + host commands

### 4.1 New `AppCommand` variants (`src/ui/command.rs`)

```rust
MacroCreate { name: String },
MacroEnsure { key: String, name: String },            // idempotent script declaration
MacroDelete { id: MacroId },
MacroRename { id: MacroId, name: String },
MacroSetValue { id: MacroId, value: f32 },         // knob + button both use this
MacroMapParam { id: MacroId, track: usize, target: ParamTarget },
MacroSetRange { id: MacroId, mapping_idx: usize, min: f32, max: f32 },
MacroSetCurve { id: MacroId, mapping_idx: usize, curve: MacroCurve },
MacroUnmap { id: MacroId, mapping_idx: usize },
// Phase 6 additions in §8.4.
```

`MacroMapParam` resolves the target's *current* value/min/max from the effect
descriptor (same lookup the `set-effect-param` dispatch already does via
`app.graph.effect_descriptors`) and seeds `range_min`/`range_max` from the
descriptor's min/max (user narrows later). It stamps the `ParamNodeId` into the
`ParamTarget` via the existing `slot_param_identity(node_id, modulator_node_id,
raw_idx)` (`scheduler.rs:428`) so the mapping is rebuild-safe.

`MacroSetValue` calls `MacroEngine::set_value` and fans `send_effective_*` over
the returned targets.

### 4.2 Host-command dispatch (`src/bin/metal_seq/main.rs`)

Register alongside `set-instrument-param` (~7929) / `set-effect-param` (~8360):

| Command | Payload | → AppCommand |
|---|---|---|
| `macro-create` | `{name}` | `MacroCreate` |
| `macro-ensure` | `{key, name}` | `MacroEnsure` |
| `macro-delete` | `{id}` | `MacroDelete` |
| `macro-rename` | `{id, name}` | `MacroRename` |
| `macro-set-value` | `{id, value}` | `MacroSetValue` |
| `macro-release` | `{id}` | `MacroRelease` |
| `macro-map-param` | `{id, kind, track, slot-idx?, param-idx, bus?}` | `MacroMapParam` |
| `macro-set-range` | `{id, mapping-idx, min, max}` | `MacroSetRange` |
| `macro-set-curve` | `{id, mapping-idx, curve}` | `MacroSetCurve` |
| `macro-unmap` | `{id, mapping-idx}` | `MacroUnmap` |

`macro-ensure` is the declarative/script-facing creation path. Keys are non-empty,
immutable, and unique within a project. If the key already exists, ensure is a
no-op and preserves the user's current display name, mappings, ranges, and value;
the supplied `name` is only the initial display name. If it does not exist, ensure
allocates a fresh monotonic `MacroId`. Deleting and later ensuring the same key
allocates a new ID; numeric IDs are still never reused.
Keys are canonicalized by trimming whitespace and a leading `:`, then ASCII
lowercasing, so `:Player/Delay-Push` and `player/delay-push` are the same key.

Follow the existing payload-extraction idiom (`map.get("…").and_then(borrow →
Number/Str)`). The `macro-map-param` payload mirrors the target descriptor
`fx-set-effect-value` already assembles in `param-controls.lisp` (bus-fx /
midi-fx / instrument branches), so the Lisp side reuses that shape; the dispatch
arm converts it into a `process::ParamTarget` (the same conversion the process
binding commands perform — share that helper).

### 4.3 Reading macro state back into the UI

The UI needs to render macro list, mappings, ranges, and live values. Follow the
project's existing state-readback mechanism (the state snapshot / reactive-field
path in `src/bin/metal_seq/state_values.rs`, as used for effect params and the
process panels). Expose:

- `macros` → list of `{id, key, name, kind, value, mappings: [{target-label, min, max, curve, current}]}`
- `macro-mapping-open` reflection is UI-local (Lisp `defstate`), not engine state.

### 4.4 Precedence: base vs macro vs p-lock vs process write (must specify)

Four things can now want the same param. The layers, from "slowest-moving" to
"most specific":

1. **Base** — `slot.defaults`, per-scene, what the knob shows.
2. **Macro override** — *live* layer on the send path (this spec). Masks base
   while engaged; between/around triggers the node holds the macro value.
3. **P-lock** — per-step, resolved at trigger time in the scheduler over the
   macro-effective default published in the immutable scheduler snapshot. Wins
   over the macro *at the triggered step* (it is the more specific,
   explicitly-sequenced intent); an un-p-locked trigger resolves back to the
   macro value.
4. **Process target write** — per-trigger transient (`Set`/`Add`,
   `apply_process_target_writes`, `scheduler.rs:3370`), layered in the scheduler
   in authored order.

The mutable override table remains command-thread-only (§3.5). Whenever it
changes, the command thread publishes a copy into the next immutable scheduler
snapshot by folding overrides into that snapshot's parameter defaults. The
scheduler never locks or reads `MacroEngine`. Consequently an un-p-locked step
keeps the audible macro value, a p-lock replaces it, and an `Add` process write
adds to the macro-effective value rather than the persisted scene base.

### 4.5 Phase 2 deliverables

- AppCommand variants + apply bodies.
- Host-command dispatch arms (+ shared payload→`ParamTarget` helper with the
  process binding commands).
- State readback for macros.
- Tests: keyed ensure is idempotent and preserves user edits; keys survive
  rename/save/reload and reject duplicates; delete + ensure allocates a fresh ID;
  map captures current value + identity; range edit; unmap releases override;
  delete releases all its overrides and renumbers nothing (IDs stable); precedence
  behaviors of §4.4 (macro vs p-lock; macro vs Add-write).

---

## 5. Phase 3 — Mapping-mode UI (highlight fork)

### 5.1 New UI state (`metal-seq-fx/state.lisp`)

```lisp
(defstate macro-mapping-open false)   ;; arm flag — global, like effect-mods-open
(defstate macro-mapping-selected -1)  ;; MacroId currently being mapped, -1 none
```

### 5.2 Wrapper fork (`metal-seq-fx/param-controls.lisp`)

Extend `param-mod-wrapper` (`:396`) and `instrument-param-mod-wrapper` (`:605`)
with a macro branch alongside the existing mods-open branch:

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
- parameters owned by any macro cannot be claimed by another macro.

Precedence: macro mapping mode and modulation mapping mode are **mutually
exclusive** — opening one closes the other. (The process-binding arm mode added
by the def-process UI is a third arm mode; fold all three into one
mutually-exclusive "arm mode" selector if the wrappers are getting crowded.)

### 5.3 Ownership display and mapping-table sidebar

Device controls retain their normal value domain during mapping; clicking the
control only selects it. Once mapped, a control shows a small green ownership
dot, reflects the macro's effective live value, and is read-only because the
macro owns it. Unmapping is performed from the mapping table.

While `macro-mapping-open`, the normal browser sidebar is temporarily replaced
by a dense **Macro Mappings** table with `Macro`, `Path`, `Name`, `Min`, and
`Max` columns. Min/max number pickers are the sole range-editing surface. The
sidebar's previous visibility is restored when mapping closes. This keeps the
script-authored player surface focused on performance controls and follows the
Ableton mapping-mode information architecture.

### 5.4 Phase 3 deliverables

- 2 new defstates.
- Macro branch in both param wrappers (green highlight, map/unmap clicks).
- Mapped-state visual (border while armed, persistent green ownership dot),
  read-only ownership, and the sidebar mapping table.
- Mutual exclusion with modulation (and process-binding) arm modes.

---

## 6. Phase 4 — Reusable macro controls + script-authored player surfaces

Macros do **not** own or require a fixed built-in panel. Provide reusable EseqLisp
components that scripts can compose into small performance interfaces, analogous
to the neural/graph sequencer demos. The project owns macro definitions and
mappings; a script owns only presentation and control layout.

### 6.1 Example script surface

```lisp
(macro-ensure :delay-push "Delay Push")

(def player-surface ()
  (column
    (macro-knob :macro :delay-push)
    (macro-momentary :macro :delay-push)
    (macro-map-button :macro :delay-push)
    (macro-mapping-editor :macro :delay-push)))
```

Re-evaluating this source resolves the same keyed project macro. It must not
duplicate the macro or reset its mappings, ranges, display name, or live value.

### 6.2 Controls

- **`macro-knob`**: continuous `0..1`, `on-change → macro-set-value {id, v}`.
  Reuse `knob-number` as in `modulator-knob`.
- **`macro-momentary`**: `on-press → macro-set-value {id, 1.0}` (or a configurable
  engage target), `on-release → macro-release {id}`. Requires press/release
  events — confirm the button widget exposes both (mod source ON/OFF buttons use
  `on-click`; the momentary needs `on-press`/`on-release` — if absent, add to the
  widget, small).
- **`macro-map-button`**: toggles `macro-mapping-open` + sets `macro-mapping-selected`
  to this macro's id. Visually "armed" while active (Ableton-style).
- **`macro-mapping-editor`**: the reusable row/table implementation mounted by
  mapping mode in the sidebar. It exposes Macro, Path, Name, min/max number
  pickers, and unmap. A future expanded editor may add the curve dropdown.
- Mapping-editor children generated with `each`, never `map` (layout tests pass with
  `map` but live rendering breaks — see repo memory `lisp-ui-each-vs-map`).

### 6.3 Where it mounts

Nowhere by default. Scripts mount these components in their own authored buffer
or tab. This deliberately leaves room for project-specific "player surfaces"
that combine macros with scene launchers, transport controls, meters, or process
controls without coupling macro ownership to the FX buffer or any device rack.

### 6.4 Phase 4 deliverables

- Reusable `macro-knob`, `macro-momentary`, `macro-map-button`, and
  `macro-mapping-editor` EseqLisp components.
- Idempotent script helper backed by `macro-ensure` and stable-key lookup through
  `SEQ.macros`.
- At least one durable experimental player-surface script/capture fixture.
- `on-press`/`on-release` on the button widget if not already present.

---

## 7. Persistence (`src/project.rs`)

Add a project-global macro list alongside the existing project-level structures:

```rust
pub struct ProjectMacro {
    pub id: u32,
    #[serde(default)] pub key: Option<String>,
    pub name: String,
    pub value: f32,
    pub kind: ProjectMacroKind,      // Mapped | Scene(ProjectSceneMacroConfig)
    pub mappings: Vec<ProjectMacroMapping>,   // empty for Scene kind
}
pub struct ProjectMacroMapping {
    pub track: usize,
    pub target: crate::process::ParamTarget,  // already serde — reuse directly
    pub range_min: f32,
    pub range_max: f32,
    #[serde(default)] pub curve: ProjectMacroCurve,
}
```

- Add `#[serde(default)] pub macros: Vec<ProjectMacro>` (plus the persisted
  `next_macro_id` counter) to `ProjectFile` so old projects load (empty list).
- **IDs are stable and never reused** even across delete (monotonic counter
  persisted). This is the seam Phase 5 depends on: a future process/graph node
  referencing "macro 3" must stay valid. Do not renumber on delete.
- **Keys are stable and unique within a project.** They are optional so macros
  created interactively remain valid, but once assigned they are immutable.
  Project load rejects duplicate keyed macros rather than resolving by order.
- `ParamTarget` persists its `param_id` (`ParamNodeId`); on load, re-resolve
  against the live descriptors and suspend/flag mappings whose target no longer
  exists — the same staleness handling process bindings and plocks already do
  (`plock_identity_matches` pattern). Suspended ≠ deleted: a mapping that fails
  in the current scene may resolve in another (§3.6).

---

## 8. Phase 6 — Scene macros ("push scenes")

The motivating use case: scene 4's delays go crazy; the player wants to *push
into* scene 4 — button held, knob controls how far — and pop back on release.
This is a macro whose mappings are **derived by diffing a target scene against
the live state**, not hand-mapped. Two independent axes, configured per scene
macro:

| Toggle | What it does while engaged |
|---|---|
| `morph_params` (default on) | Interpolate instrument/effect param values toward the target scene's values. Continuous, knob-controlled. |
| `steal_patterns` (default off) | Launch the target scene's patterns (quantized), return to the origin scene's patterns on release (quantized). Discrete. |

They are deliberately separate toggles, not a mode: params interpolate,
patterns can only switch.

### 8.1 Config

```rust
pub struct SceneMacroConfig {
    pub target_scene: usize,
    pub morph_params: bool,
    pub steal_patterns: bool,
    pub quantize: StealQuantize,      // Off | Sixteenth | Bar (default Bar)
    pub track_mask: Option<Vec<bool>>, // None = all tracks; Some = subset
}
```

`track_mask` is cheap to include from day one and immediately musical ("push
only the drum tracks into scene 4").

### 8.2 Param morph: diff at press time (live reference)

On engage (value leaves 0, or button press), the engine builds the mapping set
*fresh*:

1. Resolve the target scene's per-track param state: `ProjectScenes` cells →
   `TrackPatternData` (`effect_slots` / `instrument_slot` snapshots,
   `state.rs:451`), plus `BusPatternSnapshot.effect_defaults` for bus effects.
   This read must not clone whole patterns — read the snapshots in place (the
   pool API already supports cheap access, cf. `scene_sample_ids`,
   `state.rs:975`).
2. For each track in the mask, **structurally match** the live chain against the
   target scene's chain: same slot index + same effect (compare the snapshot's
   node/effect identity; v1 rule = same effect type at same slot index).
   Non-matching slots are skipped (see §8.3 for what "skipped" means).
3. For every matched param whose values differ beyond epsilon, synthesize a
   mapping: `range_min = current live base`, `range_max = target scene value`,
   with the interpolation domain taken from the descriptor's curve/taper
   metadata (frequencies and times must morph in their display/log domain, not
   raw units — straight-line lerp on a delay time sounds wrong).
4. Feed the synthesized mappings into the exact same override table + recompute
   loop as Phases 1–4. Knob = morph position; explicit `release(id)` →
   pop back; the synthesized mappings are then discarded.

**Live reference is locked**: the diff happens at press time against the scene
as it exists *now*, so tuning scene 4 retunes the push. (Contrast with hand-
mapped macros, which lock absolute ranges at map time — both behaviors are
right for their kind.) Persisted state for a scene macro is just the config,
never the diff.

### 8.3 What does NOT interpolate

- **Chain-structure differences** (effect present in one scene only, different
  effect type at a slot, enabled/bypass flips): v1 **ignores them entirely** —
  the push changes only shared-structure param values. This matches the
  motivating use case (same delay, crazier settings) and avoids audio glitches
  from live node swaps. A `snap_structure_at: Option<f32>` threshold (flip the
  discrete state when the knob crosses it) is a documented follow-up, not v1.
- **Patterns** — never interpolated; that's the `steal_patterns` axis. A future
  "probabilistic pattern crossfade" (per step, play the target scene's step with
  probability = knob) is a natural *process* — the process layer's per-step
  trigger writes are exactly the right home for it — and explicitly out of
  scope here. Design the knob plumbing so a process can read the macro value
  later (Phase 5), and this falls out.
- **Per-scene process chains** (`TrackPatternData.process_chain`,
  `Scene.project_process_chain`): not morphed and not stolen in v1 — the live
  scene's processes keep running. Morphing "which processes are attached" is a
  structure change (same reasoning as effects); revisit alongside
  `snap_structure_at`.

### 8.4 Pattern steal

- Engage: schedule a quantized launch of the target scene's patterns for the
  masked tracks (all tracks = the existing `"switch-pattern"` machinery,
  `main.rs:12531`; subset = per-track launch path, `"launch-track-pattern"`,
  `main.rs:12380`). The scheduler already has pending-scene handling
  (`process_runtime.clear_scene_pending`, `scheduler.rs:7374`) — reuse its
  quantization seam rather than adding a second one.
- Release: schedule the return to the origin scene's patterns, same
  quantization. Origin = the scene index captured at engage time.
- **Perf note**: full scene switches were optimized to ~60ms of host work (see
  repo memory `subtree-scene-switch-perf`) — fine at bar quantization, not
  something to fire on a 60Hz knob. This is another reason patterns are a
  discrete axis: the knob never drives `steal_patterns`, only press/release do.
- Interaction with §3.6: a pattern steal *is* a scene switch, so the restore
  pass runs — with `morph_params` also on, the morph overrides must survive it
  (they do, via the effective-send restore path). The morph diff is **not**
  recomputed on the steal switch; it was anchored at press time.

New commands: `macro-create-scene {name, target-scene}` and
`macro-scene-config {id, morph-params?, steal-patterns?, quantize?, track-mask?}`
(AppCommands `MacroCreateScene`, `MacroSceneConfig`). `macro-set-value` /
press/release are shared with mapped macros.

### 8.5 Edge rules (decide now, test explicitly)

- **Manual scene switch while engaged** → the scene macro fully disengages
  (overrides released, pending steal-return cancelled, value snapped to 0). The
  user changed the ground under the morph; popping back to a stale origin is
  worse than letting go.
- **Target scene deleted / index shifted** → config stores the scene index;
  scene delete renumbers, so on scene delete remap or clamp the
  `target_scene` of every scene macro (status-message if it got clamped).
  If scenes ever gain stable IDs, switch to those.
- **Engage while target == current scene** → no-op morph (diff is empty), steal
  is a no-op; don't error.
- **Two scene macros engaged at once** → allowed, but the later scene macro
  skips parameters already owned by another macro. Steal: last press wins the
  pattern state; each release returns to *its* captured origin — accept the
  pattern-side overlap and document it.

### 8.6 UI (extends the Phase 4 components)

A scene macro's script surface swaps the mapping editor for reusable scene controls:

```
│  ( ) Push · Scene 4      [SCENE 4 ▾]  [×]              │
│   knob: ◯ 0.00        button: ▢ (momentary)            │
│   [✓] params   [ ] patterns   quant: [bar ▾]           │
│   tracks: [all ▾]                                      │
│   diff: 14 params across 3 tracks   (live readout)     │
```

The "diff: N params" readout (computed lazily when the surface is visible) is the
main affordance telling the player the push will actually do something.

### 8.7 Phase 6 deliverables

- `MacroKind::Scene` + `SceneMacroConfig` + diff-at-engage in the engine.
- Structural-match + descriptor-domain lerp helpers (+ tests: log-domain morph,
  structure mismatch skipped, epsilon).
- Quantized steal/return through the existing scene-launch machinery.
- Commands, persistence (`ProjectMacroKind::Scene`), reusable scene controls.
- Edge-rule tests from §8.5.

---

## 9. Phase 5 seam (still out of scope, now closer)

### 9.1 Process-driven macros

Rev 1 imagined a "neural/graph sequencer" driving macro N someday. The process
layer makes this concrete and near-term: a `def-process` outlet targeting a
macro value is just a new `ParamTarget`-style destination (`Macro { id }`) plus
a marshalling hop from the scheduler to the command thread (§3.5). Everything in
this spec that Phase 5 needs is already locked: stable never-reused `MacroId`s,
single-threaded override table with a defined entry point (`set_value`), and
`ParamTarget` as the shared target vocabulary. Conversely, a process *reading*
a macro value (the pattern-crossfade idea in §8.3) needs the override/value
table snapshotted into the scheduler snapshot — the same hook noted in §4.4.
Neither is built here; do not break these seams.

---

## 10. Testing strategy

Rust (engine, the high-risk surface):
- override masks base; base edit while engaged → correct pop-back target;
  explicit release restores; zero remains engaged at the mapped minimum; curve
  math; exclusive ownership rejects a
  second macro on one param; map captures current value & `ParamNodeId`;
  stale-identity mapping suspended on reload *and* on scene switch; §4.4
  precedence (macro vs p-lock; macro vs process `Add` write); scene switch under
  engaged macro re-asserts overrides (§3.6).
- Scene macros: diff correctness (matched/mismatched chains, epsilon, log-domain
  lerp); engage/release lifecycle; §8.5 edge rules; steal quantization +
  origin-return.

Lisp/UI (layout + interaction, use the existing layout-test harness):
- script re-evaluation ensures the same keyed macro and preserves authored/user
  mappings; reusable controls resolve keys through `SEQ.macros`;
- mapping-mode green highlight appears on `:modulatable` params only;
  click in armed mode emits `macro-map-param` with correct descriptor;
  an owned parameter rejects a second macro, displays its effective value, and
  cannot be edited from its device control; mutual exclusion with mods /
  process-binding arm modes; mapping editor renders with `each` over mappings
  (never `map`).

Manual / `/run`:
- Map cutoff+feedback+size across 3 effects to one macro; sweep knob → all move;
  release momentary button → instant pop-back; reload project → mappings intact.
- Scene macro at the real use case: scene with crazy delays as target; press →
  knob morphs delay params; enable patterns → held press swaps patterns at the
  bar; release → both come home.

---

## 11. Risks / open questions

1. **§4.4 precedence** is subtler now that process writes exist — the
   "scheduler reads base, not override" asymmetry is accepted for v1 but must be
   documented and tested so it's a decision, not a surprise.
2. **Masking base edits while engaged**: decision is "macro fully owns the param
   while engaged." Confirm this matches the feel wanted vs. relative offset;
   relative-offset is a possible future curve mode.
3. **Momentary button events**: verify `on-press`/`on-release` exist on the
   button widget; if not, add them (small, but a real task).
4. **Bus-effect targets**: `ParamTarget` has no bus-effect variant yet — track
   instrument + track effects ship first; add a `BusEffectParam` variant to
   `process::ParamTarget` (benefits processes too) when bus targeting is wanted.
   Scene macros should morph bus effect defaults (`BusPatternSnapshot`) as soon
   as that variant exists — the "crazy delay" is often a bus effect.
5. **ID stability** must be honored from day one (Phase 5 depends on it).
6. **Scene-diff cost at press time**: the diff walks every masked track's
   snapshot; keep it allocation-light (it runs on a button press, budget ~1ms).
   If it shows up, cache the diff keyed on (origin scene revision, target scene
   revision) and invalidate on any param edit.
7. **Scene identity**: scene macros store a scene *index*; scene reordering /
   deletion remaps it manually (§8.5). Stable scene IDs would clean this up —
   worth doing if scenes grow more referencing features (song mode is on the
   horizon per `docs/song-mode-spec.md`).

---

## 12. Implementation order (critical path)

1. **Phase 1** — engine override layer + effective-send rewire (including the
   scene-switch restore path) + Rust tests. (Highest risk/value; everything
   else is plumbing once this is correct.)
2. **Phase 2** — commands + dispatch + state readback + tests. Persistence (§7)
   lands here (needed for reload tests).
3. **Phase 4 minimal** — stable-key `macro-ensure` + reusable knob + arm-map
   button in an experimental script surface, enough to exercise Phases 1–2
   end-to-end via `/run`.
4. **Phase 3** — mapping-mode highlight fork + click-to-map + range-edit.
5. **Phase 4 full** — mapping editor, momentary button, curve, and script helpers.
6. **Phase 6** — scene macros: diff engine + morph first (reuses everything),
   pattern steal second (touches the scene-launch machinery), scene controls last.

Phases 1–2 + minimal 4 give a testable vertical slice; 3 + full 4 complete the
hand-mapped UX; 6 delivers the "push scenes" gesture the feature was named for.
