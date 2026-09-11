# Default Process Lanes: Cirklon parity out of the box

Status: spec rev 2, 2026-09-10. Rev 1 was the plan; rev 2 records what shipped (epic eseq-ks8x) and where it deviates. Companion to
`docs/cirklon-process-accumulator-brainstorm.md` (normative process model) and
`docs/cirklon-endgame-trajectory.md`. Design canvas (approved):
https://claude.ai/code/artifact/b53be653-e637-4314-9332-23011c7be9ac

## Goal

Every track gets a small, opinionated set of Cirklon-style lanes without
touching a script. They ship as one always-on project process layer, installed
by a package the UI imports at startup. Untouched lanes are inert.

## What already exists (do not rebuild)

- Project layer + per-track copy-on-write lanes (Phase 5B/5C).
- `(step-param :retrig)` / `(step-param :rate)` resolve as process targets
  (`scheduler/process.rs`, `midi_fx.rs`).
- `:mappable` ports armed through the macro-mapping wrapper seam (Phase 3B);
  instrument/effect panels already paint colored overlays on mappable params.
- `:process-inlet` ports + `connect!` / `process-inlet` selectors (Phase 4);
  writes inside one chain land same-tick in slot order.
- Previous-tick cross-track reads with `:steps-ago` / `:trigs-ago` (Phase 7).

## The default layer (`eseq.lanes` package)

Chain order is dropdown order. Lane names have no instance prefix.

| Lane | Kind | Cirklon analog | Definition |
|---|---|---|---|
| `prob` | gate lane 0..1, default 1 | step probability | `veto!` on failed seeded roll |
| `tacc` | float lane -24..24 | aux A/B accumulate-to-note | accumulator → `(step-param :transpose)`, wrap -48..48 |
| `reset` | gate lane | accumulator reset | resets `tacc`, `acc A`, `acc B` before the step |
| `acc A` | float lane | aux C | generic accumulator, `out :mappable`, default hint `(step-param :retrig)` |
| `acc B` | float lane | aux D | same def as `acc A`, default hint `(step-param :rate)` |
| `grab` | float lane 0..1 amount | note/transpose grab | `target-add!` of `(read (track source :transpose :steps-ago lag))`; `source`, `lag` are slot inlets |
| `rand` | gate lane `roll` | generator | seeded roll between `lo`/`hi` slot inlets, `out :process-inlet` |
| `count` | gate lane `step` | generator | up-counter with `lo`/`hi`/`wrap`, `out :process-inlet` |

Dropped from the old demo: `repeats`/`span`. Native rtrg/rate cover ratchets.

Generic accumulator (`acc A`/`acc B`) has a `mode` inlet: `accumulate` (fold
lane deltas into a running value) or `pass` (forward this fire's input
verbatim). Its `amount` inlet is a lane by default, or wired from a
generator's outlet. Pass mode is what makes `rand → acc → cutoff` a
sample-and-hold without a separate lane.

Verify before locking order: a `veto!` from `prob` must still let later
accumulators advance state (spec: the ramp continues under masked trigs).

## UI (approved design)

1. **Dropdown** lists the layer's lanes under a "PROJECT LANES" header, no
   `N name/` prefix, with a dim right column showing kind or current
   destination (`acc → rtrg`, `track 1`, `gen`). Trailing `Add lane…`.
2. **Lane strip**: when a process lane is selected, the grid draws its values
   in the process color (amber, distinct from macro/mod colors) and a small
   card appears under the dropdown, in the empty space right of `rate`:
   name + "project lane" badge, IN row, `accumulate | pass` toggle, OUT row
   with the bound target chip and a `map` button. Slot inlets (`source`,
   `lag`, `lo`, `hi`) live in this card too.
3. **Map armed**: pressing `map` uses the existing arm/highlight
   infrastructure. New surfaces that light up: the step-param labels in the
   sequencer tab (`vel … rtrg rate`), and an "OTHER LANES" chip row under the
   grid listing the other lanes' IN inlets. Instrument/effect panels keep
   their existing mappable overlays untouched. Clicking a step param or
   device param writes a `ParamTarget`; clicking a lane chip writes a
   `ProcessInlet`. Same binding store either way.
4. **Wired**: IN and OUT chips render filled when bound (`← rand`,
   `→ Cutoff`). A wire pointing at a lane above the writer draws dimmed,
   since it lands on the next fire. Lanes are drag-reorderable in the
   dropdown to fix that.
5. **fx panel**: PROJECT-badged process rows are hidden. The strip is the
   editing surface for default-layer slots. Track-level user processes still
   show.

## Out of scope for launch (follow-ups)

- `length!`: set the *own* track's pattern length at the next cycle
  boundary. Track 2 reads track 1's accumulator outlet and sets its own
  length. Pull, never push; no cross-track writes.
- Fan-out: `bindings[port]` is `Option<ParamTarget>` today, so one OUT binds
  one target. Stack two accumulators until fan-out lands.
- Conductor/"player" packs as browser tabs (end-game doc).

## Persistence

The layer is persisted with the project (5B). On project load, install the
layer if missing; on package re-evaluation, reconcile by instance name so
lane edits and manual bindings survive.

## Implementation notes (rev 2, shipped)

Where the build differs from the rev 1 plan, and why.

- **Classes live in `content/processes/builtin.lisp`**, not a separate
  `eseq.lanes` package. The builtin library is the package layer that
  survives a project switch (`mark_package_defs`); a UI-imported package's
  defs would be wiped by `clear_project_authored_processes` on every load.
  Class names carry a `lane-` prefix (`lane-prob`, `lane-acc`, `lane-reset`,
  `lane-grab`, `lane-rand`, `lane-count`) because every def-process name is
  also a constructor native and `rand` / `count` are taken. Instance names are
  the user-facing lane names.
- **Installation is Rust-side per scene**: `crate::process::default_project_layer`
  / `ensure_default_project_layer` run at every scene construction site
  (`state/scenes.rs`: rebuild from snapshot, the empty first scene, and
  `insert_scene`). Missing lanes are appended, existing slots keep their lane
  edits, bindings and order. A script's `(processes :project ...)` still
  replaces the whole layer; that is the author's explicit choice.
- **One accumulator class, three instances.** `tacc`, `acc A`, `acc B` are
  all `lane-acc` with manual `out` bindings (transpose / retrig / rate) installed
  by the layer. `lane-acc` is a raw def-process with a `value` state cell, a
  `mode` inlet (0 accumulate, 1 pass), a non-lane `reset` gate inlet and
  `lo`/`hi` wrap bounds. It is *not* a `def-accumulator`: that form is a pure
  fold over the lane and cannot take a wired reset or a pass mode. Consequence:
  state advances on fired steps only (processes are invoked per trigger), so a
  step the user has switched off does not advance the ramp. A `prob` veto does
  not stop later slots, so a vetoed step still advances it.
- **Slot ids are small and exact.** Default lane slots use `(1 << 47) + index`
  rather than the 64-bit name hash: instance ids cross into Lisp as f64 and a
  hash id rounded on the way back, so lane and inlet edits silently missed.
  Override identity is still the name-derived id. `ensure_default_project_layer`
  renumbers slots saved under the old scheme and repoints the reset wires.
- **Retrig repeats follow resolved values.** Sampler and modulator retrig
  bursts (`RetrigTarget::Step`) used to re-read transpose, velocity and speed
  from stored step data on every repeat, so a process write reached only the
  first hit. The target now carries the resolved values from the initial hit.
- **Lane scope.** The strip shows NOW (the lane's current state) and a
  64-fire history graph. `ProcessRuntime` keeps a per-runtime-instance ring
  of every numeric state cell, the lookahead publishes it once per chunk
  that fired (`publish_process_scope_values`), the reactive tick mirrors it
  into `SEQ.track-process-scopes` (per track, per slot, first declared state
  cell), and the strip draws it with `linegraph` bounded by the slot's
  `lo`/`hi` inlets when present. Project slots scope per track because their
  runtime state is per track.
- **Per-track configuration (rev 2 addendum).** The per-track override
  record (`ProjectSlotOverride`) now carries bindings and scalar inlets as
  well as lanes, so a map or a `lo`/`hi`/`mode` edit made from one track's
  strip forks that track only, Cirklon-style. The strip header toggles
  `this track` / `all tracks`; `all` passes `:all` to the edit natives, which
  write the shared slot (and a shared clear drops every track's fork of that
  port). Clearing a forked binding reverts the track to the shared one. The
  old on-disk form (bare lane map) still loads.
- **Fan-out (rev 2 addendum, closes eseq-elru).** A port keeps one primary
  binding (raw value, add/set as the process wrote it) plus a list of
  `ProcessPortFanout {target, lo, hi}` entries. Each entry rescales the port
  value from the slot's output range (its `lo`/`hi` inlets, else 0..1) into
  `lo..hi` and *sets* the target. Mapping onto an already-bound port adds a
  fan-out entry; the strip lists them with editable lo/hi and a remove
  button. Per-track forks carry whole-port fan-out lists.
- **Disconnect (rev 2 addendum).** `bindings` cannot say "drive nothing":
  an absent or `None` entry means "follow the definition's target hint",
  which is how the seeded lanes (`rand` → `instrument:sr`) get their default
  target. `TrackProcessSlot::unbound_ports` (and the same set on
  `ProjectSlotOverride`) is the explicit off switch: a port listed there
  skips both its binding and its hint at fire time. Fan-out rows are
  separate targets and keep running. `seq-unbind-process-port` sets it
  (per track, or `:all` for the shared slot plus every fork); binding the
  port again or clearing it lifts it. The lane strip's OUT row carries an ×
  for this, and the `wire` port (what a lane-to-lane map binds) gets its own
  WIRE row with an × while bound, so a writer shows where it goes and not
  only the reader's `← writer` chip.
- **Lane slider drags** ride one history gesture (`apply_process_lane_drag_step`):
  scene structure is captured once at the first drag event and committed once
  when the gesture ends. Per-event capture stalled the scheduler.
- **Digit routing**: the row-wide soft number edit yields when any other
  number picker has focus; the row picker writes the whole selection when one
  exists. Lane ranges follow the slot's `lo`/`hi`. Track-typed inlets (grab's
  `source`) are a track dropdown.
- **Shared reset is wiring.** `lane-reset` has three `:process-inlet` ports
  (`a`, `b`, `c`) that the layer binds to the `reset` inlet of `tacc`, `acc A`,
  `acc B` by instance identity. Same-fire because `reset` sits above the
  accumulators in chain order.
- **Generators expose two output ports.** A port is either parameter-mappable
  or process-connectable, never both (locked decision), so `lane-acc`,
  `lane-rand` and `lane-count` write the same value to `out` (mappable) and
  `wire` (connectable). The strip's map button arms `out`; clicking an
  OTHER LANES chip binds `wire`. `seq-bind-process-port` now accepts
  connectable ports with process-inlet targets.
- **Generators are triggers, not sample-and-hold (2026-09-10).** `lane-rand`
  writes only on a roll step and `lane-count` only on a nonzero step; quiet
  steps send nothing to either port. The old write-every-fire behaviour made
  a wired accumulator add the held value on every step (rand roll
  `0 0 0 1 0 0 0 0` → a staircase, not a spike). Both keep a `hold` inlet
  (default 0) that restores sample-and-hold for a direct parameter target
  that should keep the last value across quiet steps.
- **Dropdown**: default lanes show as their instance name with no index; other
  lanes keep `N class/inlet`. The dropdown widget takes plain strings, so the
  planned section header and kind column are not there. `Add lane…` is not
  built; attaching a script still goes through the script picker.
- **Reorder** is two buttons in the strip header (▲ ▼ via
  `seq-move-process-slot-before`), not drag in the dropdown.
- **Theme slots** `process-lane-accent` (lane fill, chips, strip text) and
  `process-map-arm-bg` (armed targets) were added to the Theme struct and the
  five complete themes. The step-param tab tint paints through
  `:background-color`; `:bg` on that box is not painted.
- **Wired chip rendering**: no SDF border on the small chips (a thin border on
  a small rounded box floods it with the border color); filled vs tinted
  fills instead. Backward wires draw at reduced alpha.
- Tests that asserted an empty project layer now strip default lanes first
  (`without_default_lanes` in `state/tests.rs`, `non_default_lane_entries` in
  `ui/state_values/tests.rs`).
- Baseline (2026-09-10): the full `cargo nextest run -p sequencer` suite had
  44 failures on this tree versus 102 at clean HEAD in a worktree without the
  fetched compiler. No failure involving process chains or lanes is new; four
  non-process tests fail here but not in that worktree (conv_reverb wet arm,
  filter_table_causal click, custom_ui moved-folder dispatch,
  read_mod_display_values) and are believed environmental, not verified.
