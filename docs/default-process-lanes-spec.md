# Default Process Lanes: Cirklon parity out of the box

Status: spec rev 6, 2026-09-15. Rev 1 was the plan; rev 2 records what shipped (epic eseq-ks8x) and where it deviates; rev 3 (eseq-38k8) gives `grab` the Cirklon replace semantics; rev 4 is the lane patchbay (eseq-jrab), rev 5 bus-send targets (eseq-jmi9), rev 6 user-added track lanes (epic eseq-53y7); rev 7, 2026-09-15, per-bar transpose and the `+B` family (epic eseq-m14x). Companion to
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
- Same-tick pattern reads `(track n :param :pattern)` (rev 3): the authored
  value on the step the source track is currently on, recorded in the chunk
  pre-pass before any process runs, held until the source's next boundary.
  `:note` (chord base note, else the transpose p-lock) is only readable this
  way, as is `:note+b` (rev 7: that same note plus the source step's bar
  transpose, Cirklon `nte+B`); `(step-note)` is the same quantity for the
  running step as plain `:note`, excluding the bar, and
  `(+ (current-note) (- new (step-note)))` is the replace-the-note write that
  keeps accumulator offsets and moves chords as a block.

## The default layer (`eseq.lanes` package)

Chain order is dropdown order. Lane names have no instance prefix.

| Lane | Kind | Cirklon analog | Definition |
|---|---|---|---|
| `prob` | gate lane 0..1, default 1 | step probability | `veto!` on failed seeded roll |
| `tacc` | float lane -24..24 | aux A/B accumulate-to-note | accumulator → `(step-param :transpose)`, wrap -48..48 |
| `reset` | gate lane | accumulator reset | resets `tacc`, `acc A`, `acc B` before the step |
| `acc A` | float lane | aux C | generic accumulator, `out :mappable`, default hint `(step-param :retrig)` |
| `acc B` | float lane | aux D | same def as `acc A`, default hint `(step-param :rate)` |
| `grab` | gate lane | Cirklon grab (Inter Track) | `target-set!` replaces `value` (note / vel / dur / note+b picker) with the source track's current-step pattern value via `(read (track source :note :pattern))`; same tick, no scaling. `source` is a slot inlet. `note+b` (index 3, rev 7) reads `:note+b` instead, so the source's bar transpose comes along. Rev 3, 2026-09-14: replaced the rev 2 additive `amount × (read … :steps-ago lag)` form, which was an invention with no Cirklon analog |
| `xpose` | gate lane | Cirklon "xpose by trk n" (Inter Track) | `target-add!` of `(read (track source :note :pattern))`: this note plus the source's current-step note from the root, same tick. Appended after `roll` (rev 3) so the index-based default ids of the logic lanes do not move |
| `xpose+b` | gate lane | Cirklon "xpose by trk n+B" (Inter Track) | `lane-xpose-b`: `target-add!` of `(read (track source :note+b :pattern))`, i.e. the source's current-step note *as heard*, after its own bar transpose. Appended last in `DEFAULT_LANES` (rev 7) — the list is append-only because default lane ids are index-based |
| `rand` | gate lane `roll` | generator | seeded roll between `lo`/`hi` slot inlets, `out :process-inlet` |
| `count` | gate lane `step` | generator | up-counter with `lo`/`hi`/`wrap`, `out :process-inlet` |
| `cmp A` / `cmp B` | float lane `a` (input, or wired) | logic | 1/0 from `a` under `op` (`< > >= <= == !=`) against the `value` picker; `hold` 1 keeps the last wired input on quiet fires |
| `veto` | gate lane | logic | `veto!` on a high step, painted or wired |
| `roll` | gate lane, `rate` enum | logic | `roll!`: sequence-roll the project from the step for the step's duration |

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
- **Per-track bypass (rev 2 addendum, eseq-1ulv).** `ProjectSlotOverride`
  also carries `enabled: Option<bool>`, so bypassing `prob` from one track's
  strip or patch-bay box forks that track only; the shared slot and every
  other track keep running. The lane strip header has an `on`/`off` button
  and each patch-bay box a dot beside its name (filled while the lane runs
  on this track). Both call `seq-set-process-slot-enabled`, which forks by
  default and takes `:all` under the `all tracks` scope: that flips the
  shared slot and drops every track's `enabled` fork. A fork that agrees
  with the shared slot collapses instead of pinning a redundant value. The
  flag lives in pattern data like every other slot edit, so it is per
  pattern and undoable.
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
  when the gesture ends. Per-event capture stalled the scheduler. Since
  2026-09-11 `set_process_lane_value` also publishes without bumping
  `pattern_epoch`: the epoch is the scheduler's "destructive edit" signal
  (queue clear, re-seek, accumulator reset), and firing it per mouse event
  silenced playback for the whole drag. Lane values are content, read from
  the fresh snapshot at the next fire like a p-lock drag; steps already in
  the lookahead window keep the old value.
- **Digit routing**: the row-wide soft number edit yields when any other
  number picker has focus; the row picker writes the whole selection when one
  exists. Lane ranges follow the slot's `lo`/`hi`. Track-typed inlets (grab's
  `source`) are a track dropdown; enum inlets (grab's `value`) a picker.
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
- **Play from stopped resets process state (2026-09-11).** Every def-process
  `:state` cell (lane accumulators' `value`, rand's `held`, count's `count`,
  authored state) is cleared on the scheduler's stop→play transition and on
  an all-tracks accumulator reset, via
  `ProcessRuntime::reset_step_process_states`. Before this only the legacy
  per-track accumulator reset there; lane accumulators carried on from
  wherever Stop caught them. `reset_transport` still leaves state alone on
  purpose: scene and pattern switches call it and accumulators ride across
  those.
- **Dropdown**: default lanes show as their instance name with no index, and
  so do lanes the user added to the track through the + cell (`grab 2`, the
  minted roster name; the inlet is appended only when the class carries more
  than one lane). Script-authored chain lanes keep `N class/inlet`. The
  roster/default split comes from the `:roster` flag on the published lane
  entry (`is_track_roster_slot`). The dropdown widget takes plain strings, so the
  planned section header and kind column are not there. The planned trailing
  `Add lane…` entry became the + cell at the end of the patch bay grid
  instead (rev 6); attaching an authored script still goes through the script
  picker.
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

## Logic lanes (rev 3, shipped 2026-09-13; beads eseq-mtme)

Four more default lanes let the layer make trig-level decisions and drive
the project-wide sequence roll. Motivating patches: `rand → cmp A (> 0.8) →
roll` is a probabilistic roll; `acc A → cmp B (>= 8) → veto` silences the
track once the accumulator reaches 8. They are appended after `grab` so a
wire from any generator lands on the same fire (writes only flow forward
within one fire; a backward wire waits for the next, as before).

- **`cmp A` / `cmp B`** (`lane-cmp`): inlets `a` (the painted lane, and
  the wire input: a wire replaces the painted value on the fires it sends,
  exactly like acc's `amount`), `op` (an `:enum` over `< > >= <= == !=`,
  `==`/`!=` with a 1e-4 tolerance), `value` (the threshold picker), `hold`.
  Sends an explicit 1 or 0 on `out`/`wire` every fire; the first state cell
  is `hit` so the strip scope draws the output. On a fire where the writer
  sent nothing (rand on a quiet step) the painted lane is compared; `hold` 1
  compares the last wired input instead (sample-and-hold on the input).
  The lane inlet *must* be the wire input: the strip's OTHER LANES chip
  binds a writer's `wire` to the target lane's lane inlet, so a first cut
  with `value` as the lane wired rand into the threshold and `a` never saw
  anything.
- **`veto`** (`lane-veto`): gate lane; 1 = `veto!`. Painted, it is a mute
  mask; wired from a comparator, a conditional mute. Later lanes still run.
- **`roll`** (`lane-roll`): gate lane + `rate` enum (the eight transport
  roll rates, default 1/16). A high step calls `(roll! rate-index)`.
- **`(in? :name)`** is new: true when a process-inlet write (wire or
  fan-out) landed on that inlet this fire. `ProcessStepEventContext` carries
  `written_inlets` for it.
- **`:enum ("a" "b" …)` inlet kind** (`ProcessInletKind::Enum`): the value
  is the option index, stored as a number so bodies read it with `(in :op)`
  and serialization is untouched; the strip and the fx process panel render
  a dropdown from the published `options`. Wire and fan-out writes into an
  enum inlet are rounded and clamped to the option list, so `count → roll
  rate` sweeps the rates with no extra process.

**`roll!` semantics** (`RollState::engage_process_roll`, scheduler thread):

- Pulse only. The roll holds for the firing step's duration (its length on
  the track timebase times the duration parameter), then releases itself.
  A running roll ignores further `roll!` calls, including the same trig
  re-firing inside the loop window, so a roll never chains itself.
- The window is captured at the firing step's beat, not the lookahead
  frontier, with the manual sequence roll's per-track snapping (§5.1 of the
  rolling core spec). The roll owns the remap grid while it runs; the
  transport rate atomic is read again afterwards.
- Chunk granularity: the roll engages after the chunk in which the step
  fired, so a repeat boundary inside that chunk is missed; the scheduler
  block bounds that latency. Release is beat-exact: a chunk never straddles
  the deadline (the lookahead clamps `chunk_frames` to it).
- The transport ROLL button is fully automated, not a gate. If roll mode
  was off, the roll arms it and disarms it at release; roll mode the user
  armed stays armed. `sequence_rolling` mirrors the roll so the button goes
  red. Side effect: while auto-armed, grid note keys act as track-roll keys
  for that step's duration.
- Manual sequence-roll commands and ClearAll (roll mode off, stop, panic)
  cancel a process roll before they apply. The manual sequence roll does not
  record, so neither does a process roll.
- Rate is not audible unless it is finer than the step: a 1/16 step rolled
  at 1/16 repeats nothing before it releases.

## Lane patchbay (rev 4, shipped 2026-09-13; beads eseq-jrab)

Lane-to-lane wiring was hard to read from the single-lane strip. The
patchbay is a toggle (the `patch` chip in the strip header, state
`lane-patch-view` in `content/ui/sequencer.lisp`) that renders under the
step sliders and the strip: one box per composed-chain slot, two rows, read
left to right then top to bottom in fire order. Each box shows the lane's
connectable out ports on one row and its wireable in ports on the next.

- **Data**: `SEQ.track-lane-patch` (`build_track_lane_patch_value`,
  `ui/state_values/process_and_macros.rs`). Per slot: `out-ports` with
  `port-id = (track * 4096 + slot-index) * 16 + ordinal`
  (`lane_patch_port_id`; the track is folded in because every expanded
  track's patchbay shares one layout and the cable renderer keys sources by
  that number alone, so a slot-only id drew one track's cables from
  another's ports), `primary-free`, and `readers` (the primary binding then the fan-out
  entries, each resolved to a chain index with the scheduler's same-layer
  rule); `in-ports` = lane inlets + gate inlets + any inlet a reader already
  targets, each with the writer port ids; `param-ports` for mappable ports.
- **Cables and drag** are the generic patch-port machinery the mixer's mod
  ports use (`eseqlisp` `widget_interaction.rs` / `gpu_scene.rs`): out
  ports carry `:track port-id`, in ports `:dest slot-index :input ordinal
  :dest-kind "lane" :connected-sources writers`. `dest-kind "lane"` keeps the
  mixer's track self-patch guard out of the way; the drop handler rejects a
  lane feeding itself and a cable between tracks (the port id names its
  track). Backward cables (reader before writer) are allowed and land next
  fire, as always.
- **Scrolled drops** (fixed in `eseqlisp` `widget_interaction.rs`,
  `active_layout_pos`): the patch-drag drop and the cable click added the
  buffer's text scroll unconditionally, while widget hit-testing ignores it
  for UI-only buffers such as the sequencer. Once the sequencer had
  scrolled, the mouse-down armed the port under the pointer but the drop
  landed rows below and cancelled. The mixer never scrolls, so the mod
  ports hid it. `lane_patchbay_drag_wires_ports_through_the_real_handlers`
  drives the top-level tiled mouse path, backwards and scrolled.
- **Multiple cables per out port**: the first cable fills the port's primary
  binding (`seq-bind-process-port`); every further cable is a fan-out entry
  on that port (`seq-add-process-port-fanout`) with the identity range, which
  `ProcessPortFanout::scaled` now passes through unscaled (rev 4 fix: a
  writer without `lo`/`hi` defaulted to a 0..1 source range and clamped).
  The scheduler already routed process-inlet fan-out targets through the
  inlet-write path, so no engine change beyond the pass-through.
- **Select / remove**: cable click selects (local `lane-patch-selected`),
  the `× cable` chip removes it (clear the primary binding or remove the
  fan-out entry). The edit scope chip (this track / all tracks) applies.
- **Captures draw cables**: `render_frame_into_texture` in the Metal backend
  now runs the same global patch-cable pass as the live tiled renderer, so
  `metal_seq capture` shows mixer mod routes and lane cables (it did not
  before). Fixture: `crates/sequencer/ui/capture-fixtures/lane-patchbay.lisp`,
  which wires track-layer lane instances through `:connect` because the
  capture harness does not apply the edit natives' history commands.
- **reset collapsed to one port**: `lane-reset` had three out ports
  (`a`/`b`/`c`, one accumulator each) because a port held one target. It
  is now one `wire` port: primary binding → `tacc`, two identity fan-out
  entries → `acc A` / `acc B`. `ensure_default_project_layer` drops the
  stale `a`/`b`/`c` bindings from saved projects and installs the fan-out.
- **Box click selects the lane**: `lane-patch-select-lane` maps the slot to
  its first lane entry index and sets the track param mode the way the
  dropdown does; the strip's selected lane tints its box.
- **Backspace / Delete** remove the selected cable: `handle-key` in
  `sequencer.lisp` tries `lane-patch-delete-selected` before step deletion,
  so the keys keep their old meaning when no cable is selected.
- **Not built**: dimming backward cables (cable color is computed in Rust
  from the in-port index, so the in port carries a ↑ marker instead);
  scrolling when many track-layer lanes overflow the width.
- Baseline (2026-09-10): the full `cargo nextest run -p sequencer` suite had
  44 failures on this tree versus 102 at clean HEAD in a worktree without the
  fetched compiler. No failure involving process chains or lanes is new; four
  non-process tests fail here but not in that worktree (conv_reverb wet arm,
  filter_table_causal click, custom_ui moved-folder dispatch,
  read_mod_display_values) and are believed environmental, not verified.


## Bus-send targets (rev 5, shipped 2026-09-14; beads eseq-jmi9)

A mappable port can now drive a track's mixer send. `ParamTarget::BusSend
{ bus }` addresses the bus by project-stable id (never graph node id) and,
like every other target, writes on the process's own track.

- **Arm + click.** While a port is armed, the mixer strip and track panel
  send knobs of that track (and only that track: clicking another strip's
  send would silently bind this track's send) take the amber map overlay;
  a click binds `(dict :kind "bus-send" :bus-id N)`. `device-param` ports
  accept it alongside instrument / effect / MIDI-FX params.
- **Scheduler.** `process_apply_bus_send_write` builds on exactly what
  `resolve_track_send_params` would schedule for the step (send p-lock,
  else the live mixer baseline, else the snapshot amount), applies
  Set/Add, clamps to 0..1 and pins the left/right runtime targets as
  plain values (`live_value: None`) so dispatch does not re-read the live
  cell and undo the write. The mixer knob stays the editable base.
- **Routing edge.** The scheduler can only address a send the track lists.
  `bind-port` / `add-fanout` with a bus-send target first push a zero
  `TrackSendSnapshot` for that bus (all active tracks under `scope: all`)
  through `SetTrackSends`, outside the recorded scene-structure mutation so
  the graph edit keeps its own history entry. A bus the track still does
  not route to is traced as `bus-send-not-routed` and skipped.
- Sends are not macro-mappable through this variant: `MacroParamKey::from_target`
  returns `None` for it, like step params.
- **Effective-value dot.** The write is recorded as `ProcessEffectiveSend`
  (bus, base, value, clamped) in the overlay and published per `(track, bus)`
  through `publish_process_effective_sends`, sharing the instrument feed's
  version counter. The UI tick republishes it as
  `track-{t}-bus-{b}-send-proc-value`; process-chain edits republish
  `…-send-proc-mapped` (1 while an enabled slot binds or fans out to that
  bus). The mixer send knob gates its `process-value` amber dot on the
  mapped flag, so an unbound send drops the dot without waiting for a write.

## User-added track lanes (rev 6, epic eseq-53y7, 2026-09-15)

One process instance does one job. The rev 3 `grab` has a single `source`
track and a single `value` picker, so grabbing a note from track 1 and a
velocity from track 3 needs two grabs. The same holds for every generator
and comparator. The project layer is the wrong place to put the second one:
it is shared by every track, and the user wants the extra instance on one
track (2026-09-14). So a track carries its own added lanes.

- **Model.** A track-level *roster* — `TrackLaneRosterSlot { instance_id,
  instance_name, class_name }`, `TrackLaneRoster = Vec<…>`, all of them in
  `PatternState.track_lane_rosters` (`runtime/process.rs`,
  `state/core.rs`) — owns the *structure* of the track's own chain: which
  instances it carries and in what order. It is scene-independent. Each
  pattern's `TrackProcessChain` keeps everything else: lane values, inlet
  literals, bindings, fan-out, `unbound_ports`, `enabled`. This is the same
  split as the project layer's shared slot versus per-track
  `ProjectSlotOverride`, one axis over.
- **Identity by id band.** `TRACK_ROSTER_INSTANCE_ID_BASE = 1 << 46`,
  `TRACK_ROSTER_INSTANCE_ID_END = 1 << 47` (where the default-lane block
  starts). Both are exact in f64, which instance ids must be because they
  cross into Lisp as numbers — the same reason default lanes abandoned the
  name hash. `is_track_roster_slot` is how reconciliation tells a roster
  slot from a slot a `(processes :track …)` form authored: the latter is
  outside the band and is never touched. Runtime ids for roster slots key
  on the band id, not the usual `class:name` hash, because names are minted
  per track and two tracks can each hold a `grab 2`.
- **Reconcile.** `reconcile_track_lane_roster(chain, roster)` appends every
  missing roster slot at the END of the chain in roster order (track slots
  always run after the project layer, see
  `compose_effective_process_chain`), keeps an existing slot's
  pattern-owned data untouched, drops roster-owned slots the roster no
  longer lists, and refreshes class/display name from the roster. It runs
  in both `TrackPatternData` activation funnels (`apply_to`,
  `restore_to_impl`), so a pattern stored before a slot was added still
  activates with it; `reconcile_track_lane_roster_everywhere` walks every
  stored Patch entity (take chunks share one) plus the live chain on
  add/remove; `install_track_lane_rosters` does the same on project load.
  Idempotent by construction.
- **Track topology.** Delete-track shifts rosters down with the track
  (`topology.rs`, next to the solo-bit shift) and pushes an empty roster at
  the end; `clear_live_track_lane` clears the one track's roster with its
  chain.
- **Persistence.** Project file version 10 → 11: `track_lane_rosters` on the
  wire record, skipped when empty; a v10 file loads with empty rosters and
  is unchanged.
- **Naming.** `mint_track_roster_instance_name` takes the bare class-derived
  lane name (`lane-grab` → `grab`) when free, else `name 2`, `name 3`, … .
  `taken_track_roster_instance_names` seeds the taken set with every
  `DEFAULT_LANES` name, which the project layer owns on every track, so a
  track's first added grab is `grab 2`.
- **Ordering.** Roster slots run after all project lanes, in roster order.
  Reorder inside the track chain is still the strip's ▲ ▼
  (`seq-move-process-slot-before`). Open: that command rewrites one
  pattern's chain, so a reordered roster slot can sit at a different
  position per scene while the slot *set* is scene-independent —
  eseq-53y7.5 decides whether order belongs on the roster too.
- **Surface.** `(seq-add-track-process-slot track class-name)` → instance
  id, and a roster-aware `seq-remove-process-slot` that removes a roster
  slot from the roster (and therefore every scene) rather than from the
  active pattern only, keeping today's behaviour for project-layer slots
  (bead eseq-53y7.2). The + cell at the end of the patch bay grid opens a
  class picker over `DEFAULT_LANE_CLASSES` plus library process defs, calls
  the native and selects the new lane (bead eseq-53y7.3).
- **Gotcha (found in .1): no `MutexGuard` temporary in a `for` header.**
  `for track in 0..self.pattern.track_lane_rosters.lock().unwrap().len()`
  holds the guard for the whole loop body, and the reconcile inside locks
  the same mutex — instant deadlock. Bind the `len()` to a local first. The
  same shape lurks anywhere a `PatternState` field is read to drive a loop
  that then edits state.

## Bar transpose (rev 7, shipped 2026-09-15; epic eseq-m14x)

Cirklon P3 patterns carry one XPOSE value per bar (manual 3-14, "Bar
Values": each bar of the pattern transposes up or down by up to five
octaves), separate from the scene-level transpose. eseq now has the same
quantity, and the `+B` reads exist so one track can transpose by another
track's note *as heard*.

**Model.** Per track, per pattern, scene-locked exactly like step data. One
semitone value per 16-step page: `BARS_PER_PATTERN` = 16 (`MAX_STEPS` 256 /
16), `bar_of_step(step) = step / 16`, range ±60 (`BAR_TRANSPOSE_LIMIT`, five
octaves). `BarTransposeData` in `sequencer/data.rs` holds the values as
atomics next to `TimebasePLockData`; `PatternState.bar_transposes` is the
live copy, `TrackPatternData.bar_transpose_snapshot` makes it swap with the
pattern, and `SequencerTrackSnapshot.bar_transposes` carries it to the
scheduler. Project file v11 → v12: `bar_transpose_snapshots`, skipped when
every value is zero, so untouched projects are byte-identical and older
files load zeros.

**Where it applies, and why there.** `lookahead.rs` adds the bar value to
`resolved.transpose` immediately *after* the live print overrides and
*before* the process chain runs.

- After the print overrides because the transpose print override *assigns*
  rather than adds: folding the bar in first would have been discarded on
  any step passing under an armed print latch, and the printed value and the
  next pass would disagree.
- Before the chain so the bar is part of the note every process sees:
  `(current-note)` includes it, a `grab` note write
  (`(+ (current-note) (- new (step-note)))`) keeps it because `(step-note)`
  is authored-only, and `tacc` sums with it rather than replacing it.
- Chord steps move as a block: the chord math is already expressed relative
  to the step transpose, so every chord note shifts together.

**What stacks.** Scene transpose still applies on top, unchanged, including
the per-track `global_transpose` opt-out. Step transpose, bar transpose,
process transposes and scene transpose all sum.

**What it does not touch.** Live roll hits, live keyboard/musical-typing
streams, and neural-derived events get no bar transpose — it is a property
of an authored pattern step, not of the track.

**What reads it.** `ProcessStepPattern.bar_transpose` records it in the
chunk pre-pass; `note_with_bar()` is the sum. The track read param `:note+b`
exposes it, in `:pattern` mode only (like `:note`). Plain `:note` and
`(step-note)` stay authored and exclude the bar — that difference is the
whole point of the `+B` family, and is what makes `grab`'s `note` vs
`note+b` the manual's `nte` vs `nte+B`.

**UI** (bead eseq-m14x.2): a per-bar number picker under each page button of
an expanded track, `trn`-style formatting, undoable, per scene.

**Open follow-up** (eseq-m14x.4): pattern duplicate/halve does not copy bar
transposes yet.
