# Track Groups Spec

## Goal

Let a user fold a set of related tracks (e.g. ten drum samples that make up one
"drum sound") into a single collapsible **group** in the mixer, select the group
as one unit, and apply effects to the whole group — like Ableton's group tracks.

The motivating problem: building a rhythm often takes ~10 sample tracks, and the
mixer becomes a wall of ten full channel strips for what is conceptually one
instrument. The current workaround — manually collapsing each track and routing
them to a bus — works mechanically but does not feel like a group: there is no
single header, no one-click collapse of the whole set, no multi-select, and no
sense that the tracks belong together.

Target interaction:

```text
1. cmd-click several track strips in the mixer to multi-select them
2. cmd+g  -> the selected tracks become a group
3. the group renders as a colored header strip with its members bracketed beneath
4. a collapse button on the group hides the members, leaving just the group strip
5. selecting the group strip shows the group's effect chain (applied to all members)
```

## Non-Goals

- Nested groups (a group inside a group). V1 is one level deep.
- Reordering tracks by dragging them in/out of groups via the mixer. Membership
  is edited by the group/ungroup commands, not drag-and-drop, in v1.
- A new audio routing primitive. Groups reuse the existing bus machinery for all
  audio, fader, and effect behavior — see "Audio Model".
- Group automation / per-step group p-locks beyond what the backing bus already
  offers.
- Group support in the grid view. V1 is mixer-only; the grid continues to show
  individual tracks.

## Design Decision: bus-backed group + render-order layer

This is the load-bearing decision and it shapes the rest of the spec.

A group is **metadata over existing tracks plus an auto-created backing bus**. It
is *not* a new container that owns tracks, and it does **not** reorder the track
`Vec`.

Why not reorder the track `Vec` so members are physically contiguous (the
"folder" model)? Because per-track state is indexed by track position across many
parallel `Vec`s — `patterns`, `track_params`, `effect_chains`, `midi_fx_slots`,
`step_data`, `chord_data` in `PatternState` (`src/sequencer/state.rs:1866`).
Reordering tracks means moving all of them in lockstep without desync, and the
project's own history (see `docs/bus-routing-and-gate-sequencer-spec.md:44`)
explicitly warns that "tracks became hard to delete because too much state was
indexed directly by track position." Groups must not repeat that mistake.

Instead:

- Tracks keep their existing stable indices. Grouping never moves a track.
- A group carries an **ordered list of member track indices**.
- The mixer builds a **render order** that interleaves loose tracks and group
  blocks, mirroring the existing `mixer-v2-display-bus-index` indirection that
  already decouples bus *display* order from bus *storage* order
  (`metal-seq-mixer-v2.lisp:661`).
- Audio, fader, mute/solo, and effects for the group are provided by an
  auto-created backing **bus** (`ProjectBusChannel`, `src/project.rs:56`).
  Members route into that bus via the existing `TrackOutput::Bus(id)`
  (`src/sequencer/data.rs:26`).

Consequence: "apply effects to the group" is nearly free — it is the bus effect
chain that already exists. The real work is multi-select and the render-order
layer.

## Data Model

### Group record

A group is a lightweight, ID-stable record stored alongside tracks and buses in
the project. It does not own track data; it references tracks by index and owns a
backing bus by `BusId`.

```rust
pub struct GroupId(u64);

pub struct ProjectTrackGroup {
    pub id: u64,                 // stable group identity, not array index
    pub name: String,           // e.g. "Drums"
    pub color: [f32; 3],        // header tint; defaults from first member
    pub collapsed: bool,        // folded vs expanded in the mixer
    pub members: Vec<usize>,    // ordered member track indices
    pub bus_id: u64,            // backing ProjectBusChannel this group routes to
}
```

Add to `Project` (`src/project.rs:18`), defaulted for back-compat:

```rust
#[serde(default)]
pub groups: Vec<ProjectTrackGroup>,
```

`#[serde(default)]` means existing project files load with zero groups and behave
exactly as today.

### Invariants

- A track belongs to **at most one** group. Membership is a partition of a subset
  of tracks; loose (ungrouped) tracks are the remainder.
- A group has **at least one** member. Removing the last member deletes the group
  (and its backing bus — see "Ungroup / Delete").
- `bus_id` always points at a live `ProjectBusChannel`. The backing bus is owned
  by the group: it is not user-deletable directly while the group exists, and the
  user-facing bus list should not show group-backing buses as ordinary buses (or
  should mark them as group buses) so they aren't confused with `Bus A`/`Bus B`.
- A member track's `output` is `Bus(group.bus_id)` while it is in the group.
  Existing per-track sends are preserved and untouched.

### Identity, not index

Following the bus precedent, never key persistent group state by array position.
Groups are added/removed/reordered as metadata operations. Track deletion already
reindexes the track `Vec`; the group's `members` list must be updated in the same
operation that deletes a track (decrement member indices above the deleted track,
drop the deleted index, delete the group if it becomes empty). This hooks into
the existing track-deletion path (`docs/track-deletion-implementation-checklist.md`).

## Selection: single -> multi

### Current state

Track selection today is a single value: `current_track: Arc<AtomicUsize>`
(`src/bin/metal_seq/natives.rs:286`). Exactly one `track-selected-{i}` reactive
field is true at a time (`src/bin/metal_seq/state_values.rs`,
`sync_track_selection_binding_fields`). cmd-click currently drives the *delete
target* mechanism (`mixer-v2-select-track-delete-target`,
`metal-seq-mixer-v2.lisp:85`), not multi-select.

### Target state

Introduce a **selection set** distinct from the single "focused/current" track.
Keep `current_track` as the primary focus (it drives the sequencer view via
`reveal-sequencer-track`), and add an auxiliary set of additionally-selected
tracks for group operations:

```rust
selected_tracks: Arc<Mutex<HashSet<usize>>>,  // includes current_track when non-empty
```

Interaction rules (mixer strips):

- **Plain click**: select only this track. `selected_tracks = {i}`,
  `current_track = i`. (Unchanged single-select behavior.)
- **cmd-click**: toggle this track's membership in `selected_tracks`. Last clicked
  becomes `current_track`. This is the new multi-select gesture.
- **shift-click** (optional, v1.1): select the contiguous range between
  `current_track` and the clicked track in render order.

The existing cmd-click → delete-target behavior needs a new home. Options:
move delete-target onto a dedicated delete affordance, or gate it behind a
different modifier. Resolve in "Open Questions".

### Binding sync

Extend `sync_track_selection_binding_fields` so `track-selected-{i}` is true for
**every** `i` in `selected_tracks`, not just `current_track`. The mixer strip
already reads `track-selected-{i}` (`mixer-v2-track-selected-binding`,
`metal-seq-mixer-v2.lisp:22`) and renders a selected background/border
(`mixer-v2-strip-bg`, `metal-seq-mixer-v2.lisp:50`), so multiple highlighted
strips fall out for free once the set drives the bindings.

Expose the selection set to Lisp as a reactive list (e.g. `SEQ.selected-tracks`)
so the group command and any "N tracks selected" UI affordance can read it.

## Commands

All commands are host-commands dispatched from Lisp, parsed in the main event
loop (`src/bin/metal_seq/main.rs`, alongside `reveal-sequencer-track` at
`main.rs:4201`).

### `group-selected-tracks` (cmd+g)

Preconditions: 2+ tracks selected, none of which already belong to a group (or:
silently ungroup-then-regroup; resolve in Open Questions).

Steps:

1. Create a backing `ProjectBusChannel` with a fresh `BusId` and a default name
   derived from the group (e.g. "Drums"). Reuse the bus-creation path so it gets
   graph nodes, fader, meter, and an empty effect chain.
2. Create a `ProjectTrackGroup` with `members` = selected track indices in render
   order, `bus_id` = the new bus, `color` from the first member, `collapsed =
   false`.
3. Set each member track's `output = Bus(bus_id)`.
4. Select the new group (group becomes the selected channel; clear the
   multi-track selection set).
5. Invalidate UI so the mixer re-renders with the group block.

### `ungroup` / delete group

- **Ungroup**: dissolve the group but keep the tracks. Reset each member's
  `output` back to `Mix` (or to whatever the group's bus routed to). Delete the
  backing bus (reroute anything pointing at it to `Mix`, per the bus-deletion
  rules in `bus-routing-and-gate-sequencer-spec.md:87`). Remove the group record.
- **Delete group + tracks**: remove the member tracks (via the existing
  track-deletion path) and then the group and its bus.

### `toggle-group-collapsed`

Flip `group.collapsed`. Mirrors the existing per-track
`seq-toggle-track-collapsed` native (`src/bin/metal_seq/natives.rs:1480`) and its
project-backed reactive list `SEQ.track-collapsed`
(`metal-seq-track-collapse.lisp`). Add a parallel `SEQ.group-collapsed` reactive
surface so the mixer can branch on it.

### `add-to-group` / `remove-from-group` (v1.1)

Add the current multi-selection to an existing group, or pull a track out. Update
`members` and the track's `output` accordingly. Not required for v1 but the data
model already supports it.

## Audio Model

Entirely delegated to the existing bus system. No new DSP.

```text
Group member track:  instrument -> track fx -> output = Bus(group.bus_id)
Group backing bus:    summed members -> bus gate -> bus fx -> Mix
```

- The group fader = the backing bus volume.
- Group mute/solo = the backing bus mute/solo. Bus solo already includes tracks
  routed into the bus (`bus-routing-and-gate-sequencer-spec.md:318`), so soloing
  a group correctly auditions its members.
- Group effects = the backing bus effect chain. Dropping an effect on the group
  header strip routes to `add-builtin-effect-to-track` / `add-effect-to-track`'s
  bus equivalents (the bus effect path already exists).
- The bus gate sequencer is available to the group "for free" but can be defaulted
  all-on and visually subdued so a group behaves like a plain group until edited.

## Mixer UI

### Render-order layer

Today the mixer renders a flat range over tracks
(`metal-seq-mixer-v2.lisp:777`):

```lisp
(each (range 0 SEQ.num-tracks) |i|
  (subtree :key (str "mixer-v2-track-" i)
    (if (seq-track-collapsed? i)
      (mixer-v2-track-collapsed-strip i)
      (mixer-v2-track-strip i))))
```

Replace the flat range with a **render-order list** computed from tracks + groups.
The list is a sequence of render items:

```text
RenderItem = LooseTrack(i)
           | GroupHeader(gid)        ; the colored group strip
           | GroupMember(gid, i)     ; a member, only emitted when group expanded
```

Construction (pure function of `groups` + track count, analogous to
`mixer-v2-display-bus-index`):

1. Walk tracks in index order.
2. When reaching the first member of a group (the group's lowest member index, or
   a stored anchor), emit `GroupHeader(gid)`, then — if the group is expanded —
   emit each `GroupMember(gid, member)` in `members` order, indented/bracketed.
   Skip the members' own loose-track slots.
3. Otherwise emit `LooseTrack(i)`.

Because membership is sparse over the stable track `Vec`, the simplest v1 anchors
each group at its lowest member index and renders its members contiguously there,
regardless of their absolute indices. (Visual contiguity without physical
reordering — exactly the decision above.)

The mixer `each` then iterates render items instead of raw track indices.

### Group header strip

A new strip renderer, e.g. `mixer-v2-group-header-strip`, modeled on the existing
strips:

- Colored label with the group name (the Ableton "3 Group" header), using the
  group color via the existing `mixer-v2-track-color-*` tinting helpers
  (`metal-seq-mixer-v2.lisp:42`).
- A collapse/expand toggle button (the ☰ / triangle affordance) calling
  `toggle-group-collapsed`. Reuse the double-click-to-collapse precedent on
  track strips (`metal-seq-mixer-v2.lisp:594`).
- Group fader, meter, mute, solo — bound to the backing bus, reusing the bus
  strip controls (`mixer-v2-bus-strip`).
- Selecting the header selects the group channel (shows the bus effect chain),
  analogous to `mixer-v2-select-bus` (`metal-seq-mixer-v2.lisp:97`).

### Collapsed vs expanded

- **Expanded**: header strip + member strips rendered adjacent and visually
  bracketed (indent, shared color accent on the left edge), matching the
  Ableton session-view reference.
- **Collapsed**: header strip only; member strips omitted from the render order.
  This is the core decluttering win — ten drum tracks become one strip.

Members can still individually use the existing per-track collapse
(`seq-track-collapsed?`) when the group is expanded, so a member can be a compact
strip within an expanded group.

### Selection visuals

When the group header is selected, optionally highlight all member strips too
(they share the selection-set bindings). When a single member is selected, only
that strip highlights. Decide whether selecting the header implies selecting all
members for subsequent operations — see Open Questions.

## Persistence

- Add `groups: Vec<ProjectTrackGroup>` to `Project` with `#[serde(default)]`
  (`src/project.rs:18`). Old projects load with no groups.
- Backing buses persist as ordinary `ProjectBusChannel` entries; the group's
  `bus_id` links them. On load, validate that every group's `bus_id` resolves and
  every `members` index is in range; drop dangling groups defensively.
- Track deletion, bus deletion, and project migration paths must all keep
  `groups` consistent (see Invariants and the track-deletion checklist).

## Keybindings

- Rebind `C-g`. It is currently `(bind-key "C-g" "agent-open")`
  (`metal-seq-agent.lisp:322`). Point it at `group-selected-tracks` (scoped to the
  mixer mode, `seq-mixer-mode`, so it only groups when the mixer is focused).
- Give agent-open a new binding/home. The agent is also reachable via UI
  (`metal-seq-agent.lisp:130`) and `agent-open-instrument`
  (`src/bin/metal_seq/input.rs:659`), so it does not depend solely on `C-g`.
  Suggested replacement is out of scope here; pick an unused chord.

## Implementation Phases

Ordered so each phase is independently testable and the visible payoff comes
early.

### Phase 1 — Multi-select (no groups yet)

1. Add `selected_tracks: HashSet<usize>` selection set in natives.
2. cmd-click toggles set membership; plain click resets to single.
3. Relocate the old cmd-click delete-target gesture.
4. Extend `sync_track_selection_binding_fields` to light up all selected strips.
5. Expose `SEQ.selected-tracks` to Lisp.

Verifiable: cmd-click several strips, see multiple highlighted at once.

### Phase 2 — Group create + collapse (visual core, de-risks render layer)

1. Add `ProjectTrackGroup` + `groups` to the project schema.
2. `group-selected-tracks` (cmd+g): create backing bus, create group, route
   members, select group. Rebind `C-g`.
3. Render-order layer + `mixer-v2-group-header-strip`.
4. `toggle-group-collapsed` + `SEQ.group-collapsed`; collapsed = header only.

Verifiable: select tracks, cmd+g, see a group header that collapses/expands and
hides its members.

### Phase 3 — Group audio + effects

1. Wire group fader/mute/solo/meter to the backing bus.
2. Effect drop onto the group header → bus effect chain.
3. Selecting the group header shows the bus effect chain.

Verifiable: drop a compressor on the group, hear it process all members.

### Phase 4 — Lifecycle + robustness

1. `ungroup` and delete-group (+ bus cleanup, reroute to Mix).
2. Track-deletion / bus-deletion consistency for `groups`.
3. Defensive load validation; project migration.
4. Optional: `add-to-group` / `remove-from-group`, shift-click range select.

## Effort Estimate

| Piece | Lift |
|-------|------|
| Multi-select (set vs single `AtomicUsize`) | S–M |
| `ProjectTrackGroup` + persistence | M |
| `C-g` → `group-selected-tracks` command | S |
| Group collapse (mirror track-collapse infra) | S |
| Render-order layer + group header strip | M |
| Group fader/effects (backing bus) | XS — reuses bus system |

Overall: a **medium** feature. The audio half is nearly free because buses
already exist; the bulk of the work is the multi-select interaction and the
mixer render-ordering layer. No risky data migration because tracks are never
reindexed by grouping.

## Open Questions

- Where does the old cmd-click **delete-target** gesture move, now that cmd-click
  means multi-select? (Dedicated delete affordance, or a different modifier?)
- Does selecting a **group header** also select all its members for subsequent
  operations (effects/parameter edits broadcast to members), or only select the
  group bus?
- Should group-backing buses appear in the **bus list** at all, or be hidden /
  visually marked as group buses to avoid confusion with `Bus A`/`Bus B`?
- What happens when grouping tracks whose members **already belong** to other
  groups — reject, or move them? (V1 leans reject, since one-group-per-track.)
- Should member tracks remain individually **selectable/editable** inside an
  expanded group, or does the group capture interaction? (V1 leans: still
  individually editable.)
- Anchor policy for render order: anchor a group at its **lowest member index**,
  or store an explicit anchor so a group keeps its mixer position even as members
  change?

## Recommended V1

1. Add a `selected_tracks` set; cmd-click multi-selects; all selected strips
   highlight.
2. Add `ProjectTrackGroup` + `groups` to the project, `#[serde(default)]`.
3. `cmd+g` creates a backing bus, routes the selected tracks into it, and makes a
   group; rebind `C-g` away from agent-open.
4. Add a render-order layer (mirroring `mixer-v2-display-bus-index`) and a group
   header strip with a collapse toggle.
5. Collapsed group = header strip only; expanded = header + bracketed members.
6. Group fader/mute/solo/effects = the backing bus, reusing existing bus controls.
7. Keep tracks at stable indices — grouping is metadata, never a reindex.
8. Defer nested groups, drag-to-group, and grid-view groups.
