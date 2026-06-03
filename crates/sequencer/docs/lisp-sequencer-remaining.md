# Lisp Sequencer — Remaining Work (Phase 4b + polish)

Status: `def-sequencer` is fully working end-to-end from any UI file (auto-quoted `:tick`,
published to the scheduler, hot-reload by id). Macros over `def-sequencer` work via the
backtick/`,unquote` form. What remains is the **data contract** (params in, telemetry out),
a **UI panel**, and small polish items.

Reference: the big design doc is `lisp-sequencer-spec.md`; the original full plan with file
line-refs is `~/.claude/plans/create-a-plan-to-enchanted-seahorse.md` (Phase 4 section). The
checklist for every wiring site is **`grep -rn neural_networks`** — params must follow the
exact same path neural config takes through the three parallel structs.

---

## 1. `param` — UI knob/control → runtime (per-pattern, serialized)

Goal: declare controls inline in the sequencer file; read them in `:tick`; persist per-pattern.

- **lisp_effect.rs**: `(param name :kind ... :default ... :min ... :max ...)` declaration
  parser (model `midi-fx-param`). New `SequencerParamDescriptor { name, kind, default, ui_hint }`
  + `SequencerParamKind ∈ Float{min,max} | Int{min,max} | Enum{options} | String | Track |
  Timebase | Vector{len} | Matrix{rows,cols}`. Read in `:tick` via a **context-backed**
  `(param-get name)` builtin (NOT an upvalue — mirror the gen-* context pattern). Write from
  UI via `(seq-set! name field v)` (generalize `seq-set-midi-fx-param`).
- **project.rs**: `ProjectSequencer { id, name, enabled, param_values: Vec<(String,
  SerializedParamValue)> }` (mirror `ProjectNeuralNetwork`); `SerializedParamValue` covers
  float/int/string/vector/matrix. Add `#[serde(default)] pub sequencers: Vec<ProjectSequencer>`
  to `ProjectPattern`. **Round-trip test required** (old JSON loads empty).
- **state.rs + snapshot.rs**: thread `sequencers` through `PatternSnapshot`,
  `SequencerSnapshot`, `SequencerSnapshot::capture`, pattern-bank capture. Add
  `edit_current_sequencers` / `current_sequencers` (mirror `edit_current_neural_networks`).
- **scheduler.rs**: `should_reload_generators` (mirror `should_reload_neural_runtime`); on
  reload push param values into the generator context + realign.

Risk: missing any one of the three structs/capture sites silently drops params on pattern
switch. Grep `neural_networks` as the checklist.

## 2. `state` telemetry — runtime → UI (block-rate, latest-wins)

Goal: surface generator state to visualizations (the inverse of param).

- **state.rs**: `sequencer_visualization: Mutex<SequencerVisualizationSnapshot>` (mirror
  `neural_visualization`) + accessors. Snapshot = `HashMap<generator, HashMap<var, StateValue>>`
  with fixed-shape vectors/matrices/rings.
- **lisp_effect.rs**: extend `state-set!` to support non-scalar shapes (`(state-set! name idx
  v)` for vectors); add `(seq-state name var)` reactive UI read (generalize the existing
  `SEQ.neural-energy-matrix` reference).
- **generator.rs**: on the once-per-block publish path (mirror the `set_neural_visualization`
  call site in scheduler.rs) apply `:hold` (mirror `trigger_visual_until_beats` +
  `TRIGGER_VISUAL_HOLD_BEATS`) and `:decay` (mirror `apply_energy_decay`) smoothing; append
  `:ring` history. Must be lossy — never back-pressures the tick.
- **bin/metal_seq/state_values.rs**: generalize `sync_neural_visualization_fields` /
  `build_neural_energy_matrix_value` to populate `(seq-state …)` refs each UI frame.

## 3. Per-sequencer UI panel + new param widgets

- The "one file, both halves" story: `def-sequencer` + an `effect-buffer` panel in the same
  file (panel renders in the UI VM; `:tick` runs on the scheduler). Confirm the publish path
  does not depend on the form living in `*scratch*`.
- New widgets for param kinds: string → text input, vector → row of cells, matrix → existing
  `matrix` widget (already used by the neural panel). Each needs `SerializedParamValue` serde +
  a `seq-set!` write path per shape.

## 4. Small polish

- **`:init`** is currently parsed but inert. Wire it to run once when a generator is first
  registered / re-registered with an incompatible signature (the natural place is
  `GeneratorRuntime::sync_definitions` where fresh instances are created). Decide whether
  `:init` runs on every hot-reload or only on fresh/incompatible — likely only fresh, so it
  doesn't clobber live state.
- **Non-scalar state cells**: today `state-get`/`state-set!` are scalar f64 keyed by string.
  Vectors/matrices land naturally alongside the telemetry shapes in item 2.
- **Docs**: add a short note to the sequencer docs that macros over `def-sequencer` require
  the backtick/`,x` form (plain-list macro bodies are returned verbatim — params not
  substituted). This bit several times; worth a footgun callout.

---

## Verification

1. `cargo test -p sequencer` — neural golden tests unchanged; generator unit tests; new
   project.rs round-trip for `sequencers`.
2. `run` skill, chord-seq with a `param` knob: edit knob live → density changes without
   re-eval; switch patterns → param swaps and survives save/load.
3. `run` skill, telemetry: a `:tick` that `state-set!`s a viz var → matrix/vector panel
   reflects it; `:hold` holds across the interval; `:ring` bounds history.
4. The spec's Jaki-Liebezeit one-file example (string `cell` param → text input, `density`
   knob, `fire` vector → matrix viz) as the end-to-end acceptance bar.

## Known pre-existing test failures (NOT caused by this work — verified on clean baseline)

`loads_generated_catalog`, `lookup_docs_finds_mod_and_preamble_envelope_helpers`,
`patcher_insert_created_phasor_multiply...`, `spectral_notch_phaser_depth_changes_signal`,
`add_midi_fx_to_track_publishes_snapshot_without_deadlocking_pattern_bank`. Ignore these.
