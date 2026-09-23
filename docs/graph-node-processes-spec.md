# Graph node processes

Status: spec rev 3, 2026-09-21. BUILT uncommitted 2026-09-22: `.1` engine hook, `.2` natives (no `def-node :process` default yet), `.4` expanded neuron editor in `alez.neural.variable-reset` (dropdown wiring, not drag cables). `.3` neuron harmony source BUILT 2026-09-22 (negative `:source` = neuron k, `(neuron k :note|:chord|:key)` reads). Open: `.5` lane-shift + lane-choke, `.6` drag cables. Bead: `eseq-waa9` (epic; children eseq-waa9.1 eseq-waa9.2 eseq-waa9.3 eseq-waa9.4).

## 1. Problem

A neural / graph sequencer routed to a melodic instrument produces an evolving
line from per-node transposes accumulating around feedback loops. Keeping it in
key today means a `scale` MIDI fx on the destination track: a pitch quantizer
bolted onto the exit. It works but it is post hoc. The graph never hears the
corrected note, so the line drifts out of key inside the loop and gets snapped
on the way out, and every node on that track gets the same treatment.

Track processes (`lane-harmony`, `lane-prob`, `lane-veto`, `repeater`, ...) are
the right vocabulary, but they only run on pattern-step triggers. A graph
emission takes a separate path (`enqueue_emitted_network_event_with_midi_fx`)
that resolves the route, runs MIDI fx and enqueues; the process cascade in
`scheduler/lookahead.rs` never sees it. Putting `lane-harmony` on the
destination track therefore does nothing to neural notes.

## 2. Design

A node owns a process chain. It runs **at emit time on the payload**, before
the payload is either emitted to a track or scattered along the node's
out-edges. Because the graph engine uses one `GraphPayload` for both (see
`commit_firing` in `runtime/graph.rs`: `resolve_emission_payload` feeds both
`push_emission_event` and `push_outgoing_propagations`), a process that changes
the note changes what downstream nodes accumulate on. Harmony inside the loop,
not at the exit.

```
fire → resolve_emission_payload → [node process chain] → emit + scatter
                                        │
                                        ├─ transpose write  → payload.note
                                        ├─ velocity write   → payload.velocity
                                        ├─ duration write   → payload.duration_beats
                                        ├─ veto             → no emit; scatter continues; energy still zeroed
                                        └─ choke            → other nodes' emits muted while this note sounds
```

Concretely, with nodes 1, 4 and 6 all routed to track 1:

- node 4 carries `lane-prob 0.5`: half its fires are silent, but every fire
  still scatters, so the loop through 4 keeps running at full density while
  track 1 hears half of it;
- node 6 carries `lane-harmony :source (track 8)`: its note snaps to track 8's
  current chord before it sounds and before it rides on;
- node 1 carries `lane-harmony :source (neuron 3)`: it follows whatever node 3
  last played (§4);
- node 6 also carries `rand → cmp → lane-shift +7`: about a third of its
  fires are a fifth up, and because the write rides the scatter the shift
  travels round the loop (§2.5);
- node 4's `rand → cmp` also feeds `lane-choke`: when the coin lands, every
  other node is silenced for as long as node 4's note sounds (§2.6).

### 2.1 Why not the destination track's chain

Running the target track's process chain on emissions (the cheaper option)
was rejected. Lane values are indexed by pattern step, and on a neural
destination track every step is toggled off, so the lane sliders the user
would be editing belong to steps that never play. It also stays post hoc: the
graph does not hear the result. Option kept in §8 as a possible later
convenience, not as the model.

### 2.2 What the process sees

The chain runs with a synthesized step context so existing `def-process`
bodies work unchanged:

| `ProcessStepRunContext` field | Node value |
|---|---|
| `track` | the node's resolved route track (`None` route: chain still runs, emission is dropped as today) |
| `step` | the destination-track step index the fire lands on (quantized to the node's resolution); used only for lane reads, §2.3 |
| `beat`, `sample_time` | the fire's beat / sample |
| `step_beats` | the node's resolution in beats |
| `resolved.transpose` | `payload.note` |
| `resolved.velocity` | `payload.velocity` |
| `resolved.duration` | `payload.duration_beats` |
| `note` | `payload.note` (what `(current-note)` returns) |
| `event` | the emission as a Lisp value plus `:node <index>` and `:graph <name>` |

Writes flow back the same way: `(step-param :transpose)` targets edit
`payload.note`, `:velocity` edits `payload.velocity`, `:duration` edits
`payload.duration_beats`. `(veto!)` cancels the **emission only**: the node
still fires as far as the graph is concerned (energy zeroed, incoming dampened,
cycle advanced, payload scattered along its out-edges). A veto is a mute, not a
break in the chain, so a probability gate thins what a track hears without
starving downstream nodes (rev 2; rev 1 had veto also drop the scatter, which
turns a sparse loop into a coin flip on whether it survives each pass). A
chain-breaking variant, if ever wanted, is a separate verb (`(kill!)`), not a
mode on veto. Note that transpose / velocity writes made before a veto still
ride the scatter. `ratchet!` is honored on the emission only (repeats do not
scatter). Device targets (instrument /
effect / bus writes) apply as they do for steps.

### 2.2.1 The `delay` payload field (rev 4, 2026-09-22)

Besides transpose / velocity / duration, a node patch can write **`delay`**:
a signed number of node steps added to this fire's propagation delay (node or
edge delay, rounded, floored at 0). It is authored like a step param
(`:target (step-param :delay)`, `graph-node-process-map … :delay`, the
`delay` picker lights up while mapping) but never touches a `ResolvedStep`:
the runner carries it on `ProcessTargetOverlay::node_delay_offset_steps`,
`GraphDriver::take_delay_offset_steps` hands it to `commit_firing`, and
`push_outgoing_propagations` applies it to that fire only. On a track step it
is ignored. `acc` (amount 1) mapped onto `delay` makes a self-feeding neuron
wait 2, 3, 4… steps between hits; `neural-delay` is the knob/wire form.

### 2.2.2 Resets, both ways (rev 4, 2026-09-22)

`NodeEmitContext::after_reset` is true on a node's first fire after any
reset (the periodic bar reset, a fire-authored reset, or a patch's request);
`(reset-fired?)` reads it. `(graph-reset! group)` from a node patch queues
`GraphControlCommand::Reset` for the node's own graph; the runtime applies it
after the boundary's fires commit (`GraphDriver::take_reset_requests`), as a
whole-graph reset (`None`) or `GraphRuntime::reset_group` for one neural
group. The `neural-reset` builtin (`reset` on a node) is both: `fired` sends
1 on the post-reset fire (wire it into `acc.reset`), a high `trigger` resets
`group` (`all`, `A`..`D`).

### 2.3 Composability: the node chain is a patch

The node chain is not a list of slots with knobs; it is the same **process
patch** the expanded step sequencer has (the patch bay under the lane grid in
`content/ui/sequencer.lisp`): cards with inlets and outlets, cables between
them, `rand → cmp A → veto` and friends. Everything a track patch can do a
node patch can do, minus per-step lanes:

- the port-binding natives (`seq-bind-process-port`, `seq-add-process-port-fanout`,
  `seq-unbind-process-port`, fan-out ranges) get graph-node twins keyed by
  `(graph name, node index, instance id, port)` instead of `(track, instance
  id, port)`, sharing the implementation;
- a cabled inlet reads its cable, exactly as on a track. An uncabled inlet
  is a scalar knob (§2.4). So `lane-prob`'s `prob` can be a knob or a
  `dice` output; `lane-veto`'s `gate` can be a knob or a comparator;
- a process patch on a node runs once per accepted fire in card order, same
  cascade rules as a track (`invoke_process_cascade`).

The point is that gates, comparators, counters, accumulators and RNGs already
exist and already compose; nodes just need to be a place they can live.

### 2.4 Lane inlets on a node

A `:lane true` inlet has no pattern step to index on a node. Rule: **on a
node, every inlet is a scalar**. The lane slider row is replaced by one knob
per lane inlet in the node's process editor (§6). This is honest about what a
node is and avoids ghost lanes on dead steps. `:lane true` inlets keep their
default and range; only the per-step editing surface is gone.

If a patch really wants per-step variation on a node, the node's rule can
already read `(step)` and write the inlet through `target-set!` from a
`dice`-style process; that is library, not kernel.

### 2.5 `lane-shift`: conditional transpose on top of the node transpose

A node already has a `transpose` param applied on every fire. What is missing
is a *conditional* one that composes with a gate:

```lisp
(def-process lane-shift
  :doc "Add a fixed transpose when the gate is high. On a node the shift rides the scatter, so downstream nodes hear it too."
  :target (step-param :transpose)
  :in ((gate :gate :default 0 :lane true)
       (semitones :int -24 24 :default 7))
  :run (if (> (in :gate) 0.5) (target-add! (in :semitones)) nil))
```

Wired as `rand → cmp → lane-shift`, that is probabilistic shifting of the
melody. Two on one node with different comparator thresholds give a small
transition table. It is an ordinary lane process and works on tracks too
(there the gate is a lane like `lane-veto`'s).

### 2.6 `choke!`: mute every other trigger while this note sounds

A drum-machine choke group, issued from a process. `(choke! :scope s :beats
d)` mutes emissions from every *other* node in scope until `d` beats after
this fire. `d` defaults to the **emitted** duration of this hit: the choke is
resolved after the whole patch has run, so a duration write earlier in the
same patch (or the node's `dur-factor`) lengthens the choke with the note.
`(current-duration)` inside the patch reads that same resolved value. The
choke ends exactly when the hit's gate ends; it never outlives the note
unless `hold` says so. Scope is `:all` (every node in the graph), `:group`
(nodes in the choker's neural group) or `:track` (nodes routed to the same
track). The choker itself is exempt. Like `veto!`, a choke mutes emissions
only: the choked nodes still fire, scatter, zero energy and advance, so the
graph's dynamics are untouched and only what reaches the tracks changes. A
choke already running is extended, never restarted. Runtime: `GraphRuntime`
keeps a small list of `(scope, until_beats)`; `commit_firing` consults it
before `push_emission_event`. It is a `ProcessRunCommand::Choke`, the same
shape as `Roll`, and like `roll!` it errors outside a run scope that can
honor it (a track process has no graph to choke).

```lisp
(def-process lane-choke
  :doc "Choke: while the gate is high, every other node in scope is silenced for as long as this note sounds (times hold)."
  :in ((gate :gate :default 0 :lane true)
       (scope :enum ("all" "group" "track") :default 0)
       (hold :float 0.25 8 :default 1))
  :run (if (> (in :gate) 0.5)
         (choke! :scope (in :scope) :beats (* (in :hold) (current-duration)))
         nil))
```

`rand → cmp → lane-choke` gives occasional solo moments where one node cuts
through a dense graph.

### 2.7 Chain position and cost

The chain runs after max-poly arbitration (so a losing candidate never runs
its processes) and before energy reset. Cost is one Lisp cascade per accepted
fire per node with a non-empty chain; nodes with empty chains pay a length
check. The graph already runs in `process_block` on the scheduler thread next
to `scheduler.process_runtime`, so the hook is plumbing.

## 3. Data model

`ProjectGraphNodeOverride` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub process_chain: Option<TrackProcessChain>,
```

Reusing `TrackProcessChain` / `TrackProcessSlot` verbatim gives serialization,
instance identity, enable/disable, inlet values and copy/paste for free. Slots
on a node are always track-local (never project-layer): `project_slot_id` is
`None` and the project process layer does not fan into nodes.

`GraphNode` (runtime) gains `process_chain: TrackProcessChain`; the manifest
default is empty. Live config updates (`apply_config_preserving_state`) swap
the chain like they swap `route`.

Process instance state (`:state` cells, `:seed :locked` RNGs) is keyed by
`(graph id, node index, slot instance id)`. Reset on transport play from
stopped, like track slots.

## 4. Harmony sources: track or neuron

`lane-harmony` reads `(track n :chord :pattern)` and `(track n :key :pattern)`.
Both keep working on a node when the source is a track. To "follow neuron 3"
the `source` inlet accepts a neuron reference:

- Authoring: `:source (neuron 3)` in Lisp; the picker offers tracks and, for a
  node-owned chain, the graph's own nodes.
- Reads: `(neuron k :note)` = the pitch class set `{pc(last emitted note)}`
  of node `k`, from `node_events[k]`; `nil` if it has never fired since reset.
  `(neuron k :key)` = the union of pitch classes of node `k`'s last 8
  emissions (`event_history` filtered by node). Both are same-tick reads, like
  `:pattern` reads on tracks.
- `harmonic-snap` is unchanged: a one-note "chord" scores that pitch class as
  the chord tone and the derived key as key tones. With `amount 1` the
  follower lands on the leader's pitch class (any octave); with `~0.5` it stays
  inside the leader's recent pitch set.

Neuron sources are only offered on node-owned chains. A track chain reading a
neuron would need a graph name too; not needed now.

## 5. Authoring surface (Lisp)

```lisp
(def-node nrn
  ...
  :process ((lane-harmony :source (track 8) :amount 0.7)
            (lane-prob :prob 0.5)))
```

is the manifest default for every node of that class. Per-node overrides go
through the existing node-edit funnel:

```lisp
(graph-node name k :process-add "lane-prob")          ; append a slot, returns instance id
(graph-node name k :process-remove <instance-id>)
(graph-node name k :process-inlet <instance-id> :prob 0.5)
(graph-node name k :process-enable <instance-id> false)
(bind-graph name k :process)                          ; reactive list of slot plists
(bind-graph name k :process-inlet <instance-id> :prob)
```

Names mirror the track natives (`seq-track-process-add` etc.) so the process
editor code in `content/ui/` can be reused with a different target.

## 6. UI: expanded neuron editor

Each node row in the two neural demos (and their `~/.eseq.d` copies) gets an
expand toggle. Expanding a row swaps the panel into **expanded neuron mode**
for that node: the compact row grid is replaced by one node's full editor,
with the same header controls and a back / next-node / previous-node strip so
you can walk the graph node by node. The expanded editor holds:

- the node's existing per-row controls (route, group, seed, delay, transpose,
  vel decay, dampening, recovery, resolution, quantize) as labeled knobs and
  pickers with room to breathe;
- the node's **process patch**: the track expanded editor's patch bay
  reused as-is (cards, ports, cables, the slot inspector at the right with
  on/off), minus the 16-step lane grid; uncabled inlets are knobs (§2.4), the
  `source` picker offers tracks and neurons (§4);
- the node's row and column of the weight matrix (its in-edges and
  out-edges) as two labeled 1×N strips, so edges can be tuned without
  leaving the node.

A node with a non-empty chain shows a small dot on its compact row. Expanded
mode is UI state only (`defstate`), not project state.

No per-node lane editor. No changes to the track process panel.

## 7. Phases (beads)

1. **Engine hook** (`eseq-waa9.1`): `process_chain` on override + node,
   run the cascade in `commit_firing` / `commit_reset_seed_emission` with the
   synthesized context, apply transpose / velocity / duration writes and veto
   to the payload before emit + scatter. Tests: harmony write changes what a
   downstream node receives; veto stops scatter but zeroes energy; empty chain
   is byte-identical to today.
2. **Authoring natives** (`eseq-waa9.2`): `:process` on `def-node`, the `graph-node`
   verbs and `bind-graph` reads in §5, serialization round-trip.
3. **Neuron harmony source** (`eseq-waa9.3`): `(neuron k :note)` / `(neuron k :key)`
   reads, `:source (neuron k)` on `lane-harmony`, picker entries.
4. **Expanded neuron editor** (`eseq-waa9.4`): per-row expand toggle and the
   expanded-mode panel (node controls, process patch bay reused from the
   track editor, in/out edge strips) in both demos and the user package
   copies. Includes the graph-node twins of the port-binding natives (§2.3).
5. **Builtin processes** (`eseq-waa9.5`): `lane-shift` (§2.5, works on
   tracks too) and `lane-choke` + the `choke!` verb + `ProcessRunCommand::Choke`
   + the graph runtime choke list (§2.6). `lane-shift` is independent;
   `lane-choke` needs the engine hook from `.1`.

`eseq-waa9.1`, `eseq-waa9.2` and `eseq-waa9.3` are independent of each other; `eseq-waa9.4` needs `eseq-waa9.2` and `eseq-waa9.3`; `eseq-waa9.5`'s choke half needs `eseq-waa9.1`.

## 8. Not doing

- Destination-track chains on emissions (§2.1). Could return later as a
  per-track "also process routed-in events" toggle if a use case appears.
- Lane-shaped inlets on nodes (§2.4).
- Choke breaking propagation. Same reasoning as veto: mutes are for what
  tracks hear, not for the graph's dynamics.
- Sequencer-level chains ("gate group B at 40 %"): a per-node chain plus the
  group dropdown covers it by putting the same slot on every group-B node;
  revisit if the copy-to-all-nodes gesture is not enough.
- Processes reading or writing graph energy / edges. That is the homeostat
  spec's territory (`docs/graph-homeostat-spec.md`).
