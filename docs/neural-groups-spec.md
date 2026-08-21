# Neural Groups: Cluster-Level Control for Graph Sequencers

Status: draft spec, 2026-08-05. Companion to
`docs/graph-homeostat-spec.md` (the delta overlay — normative for the nudge
layer, already landed in `runtime/graph.rs`) and
`content/scripts/sequencers/graph-neural-variable-reset-demo.lisp`
(the graph sequencer this extends).

## 1. Motivation

Graph-mode neural sequencers are excellent at generating material and painful
to *steer*. Three concrete pains, all observed while working on jungle-ology
and gotham-piano:

1. **Polyphony stealing breaks loops.** With a large node field and a small
   `max-poly`, whichever nodes happen to be hot dominate the slot budget. A
   deliberately-authored loop (say the last three nodes driving hats) gets its
   fire rejected by an unrelated earlier node — and because rejection *zeroes
   the node's energy* (`drop_firing`, `runtime/graph.rs:2158`), the loop's
   circulating charge is silently erased. The loop doesn't just miss a hit; it
   dies. Today the only fix is fine-tuning weights and thresholds until the
   clusters happen not to collide, which is fragile and doesn't survive a
   scene change.

2. **The tuning surface is N².** A 16-node graph has 256 edge weights. There
   is no handle between "one edge weight" and "the whole matrix", so
   expressing an intention like *"the hat cluster should feed the kick cluster
   lightly"* means editing a dozen cells by hand, and morphing that intention
   across scenes means morphing 256 numbers.

3. **There is no way to author "who plays when".** Sustained, musical
   turn-taking between regions of the graph — call and response, a cluster
   that steps back while another takes a phrase — is currently an accident
   that emerges from careful weight tuning, if at all.

**Neural groups** address all three with one construct: a per-node group
assignment (like track mute groups, but for nodes), plus a small `k×k` control
surface over those groups. The N² problem becomes a k² problem, arbitration
becomes per-group, and turn-taking becomes an authored dynamic rather than a
lucky one.

### Design principle: inert by default

Every default in this spec reproduces today's behavior exactly. All nodes
start in group A; the coupling matrix defaults to all-ones; the activity
matrix defaults to all-zeros; `reject-retain` defaults to `0.0` (today's
zeroing). Existing projects must sound bit-identical after this lands, and the
regression bar for the engine work is exactly that.

## 2. Locked decisions

- **Groups are a flat per-node assignment**, not a nested hierarchy. Max 4
  groups (A–D) in v1.
- **Polyphony is per group, sharing one authored value.** `max-poly = 2` means
  2 slots for group A *and* 2 for group B. No per-group budget UI in v1 — the
  user explicitly asked for the simple version first.
- **Two coupling layers, not one.** `G` (gain) scales spike propagation
  between groups; `H` (activity) shifts thresholds based on recent firing
  *rate*. They do different jobs and must not be collapsed: `G` shapes where
  energy flows, `H` shapes who is ready to spend it. `H` works between groups
  with no edges at all.
- **Rebound is engine-owned, not authored per event.** The escape from mutual
  suppression is a dynamic of the group layer (§4.6), not something the user
  sequences.
- **Nothing here touches the `:update` lisp rule.** The group layer resolves
  into the params the rule already reads. Existing node rules keep working
  unmodified.
- **All group state is per-pattern**, like node/edge overrides — so it scenes
  and captures through the existing paths.

## 3. Data model

### 3.1 Node field

One new per-node intrinsic, alongside `:route` / `:delay` / `:resolution`:

| Field | Type | Default | Range |
|---|---|---|---|
| `:group` | int | `0` (group A) | `0..GROUP_MAX-1` |

`GROUP_MAX = 4` in v1. Stored as a sparse per-node override exactly like
`:delay`, set via `graph-node`, read via `graph-node-value`, bound in the UI
via `bind-graph`.

> **Naming collision — read before implementing.** `group` is already taken in
> the override structs: `ProjectGraphNodeIntrinsicOverride.group`
> (`runtime/graph.rs:533`), `ProjectGraphNodeParamOverride.group` (:561) and
> `ProjectGraphEdgeParamOverride.group` (:569) all hold the *prototype / edge-set
> name* (`self.node.name`, or `"{from}->{to}"` from `edge_set_group_id`,
> graph.rs:2950). The Rust field for this feature must therefore be
> `neural_group` (or `cluster`) on `GraphNode` / the intrinsic override struct.
> Only the lisp-facing keyword stays `:group`.

### 3.2 Group-level config

New sequencer-level config (per-pattern, `graph-config` / `bind-graph-config`
/ `graph-config-key`):

| Field | Type | Default | Range | Meaning |
|---|---|---|---|---|
| `:group-count` | int | `1` | `1..4` | How many groups are active/shown. |
| `:group-gain` | k×k float | all `1.0` | `0..2` | `G` — propagation gain, §4.3. |
| `:group-coupling` | k×k float | all `0.0` | `-2..2` | `H` — activity→threshold, §4.5. |
| `:group-trace-decay` | float | `0.5` | `0..1` | Per-beat decay of the activity trace, §4.4. |
| `:group-debt-rate` | float | `0.15` | `0..1` | How fast the debt follower tracks suppression, §4.6. |
| `:group-rebound` | float | `0.0` | `0..4` | Threshold undershoot gain on release, §4.6. |
| `:group-surge` | float | `0.0` | `0..1` | Energy surge gain on release, §4.6. |
| `:group-threshold-param` | keyword | `:threshold` | — | Which node param `H` modulates, §4.5. |

Two matrices at k ≤ 4 is at most 32 floats — small enough to serialize inline
and to interpolate wholesale when morphing scenes.

### 3.3 Node param

| Param | Type | Default | Range | Meaning |
|---|---|---|---|---|
| `reject-retain` | float | `0.0` | `0..1` | Fraction of energy kept when a fire is rejected by polyphony selection, §4.7. |

Declared in the node prototype's `:params` like `threshold`, so it takes the
existing per-node override + global-set-all treatment in the demo panel.
Default `0.0` preserves current behavior; **`0.5` is the recommended starting
value** and should be what `script-init-fn` writes for new patches.

## 4. Engine semantics

All of this lands in `runtime/graph.rs`, inside the existing per-boundary
phase structure of `process_block` (graph.rs:1363–1602). New per-group runtime
state, sized `group_count`:

```rust
activity:  Vec<f64>,   // leaky firing-rate trace, §4.4
debt:      Vec<f64>,   // slow follower of suppression, §4.6
```

Both live alongside `energy` (graph.rs:897) in the `GraphRuntime` SoA block,
are initialized in `new_from_config` (:1021–1034), cleared in `reset_internal`
(:1300–1313), and are *not* persisted.

### 4.1 Group resolution

`group(i)` resolves through the standard chain (manifest default ⊕ per-pattern
override ⊕ delta), clamped to `0..group_count-1`. A node whose group index
exceeds the active `group-count` collapses to group A rather than
disappearing, matching how out-of-range routes degrade.

### 4.2 Per-group polyphony

Today `max_poly_accept` (`runtime/graph.rs:2196`) takes the full candidate list
for a boundary and accepts `max_poly` of them under the selected
`NeuralMaxPolySelection` mode. The change:

> Partition candidates by `group(node_index)`, preserving the existing
> `(fire_sample, node_index)` sort order within each partition, then run the
> **unmodified** selection over each partition with the same `max_poly`.
> Union the accepted sets.

Consequences worth stating explicitly:

- Selection modes keep their exact semantics, now scoped per group.
  `:loudest` picks the loudest *within* each group; `:seed-first` prioritizes
  seeds within each group.
- Determinism is preserved: partitioning is a stable filter over an
  already-sorted list, so the accepted set is a pure function of the candidate
  list and the group assignment.
- With `group-count = 1` this is byte-identical to today.
- Total simultaneous voices can now reach `group_count × max_poly`. That is
  intended. §9 raises whether an optional global ceiling is wanted.

This alone fixes pain (1): a loop in group B can no longer have its slot
stolen by group A, because they never compete.

### 4.3 `G` — propagation gain between groups

When an edge `i → j` contributes to node `j`'s input accumulator, its
contribution is scaled by `G[group(i)][group(j)]`:

```
contribution = gather(edge) * G[group(from)][group(to)]
```

Applied in the gather fold where `(edge :weight)` is resolved (deposit phase,
Phase 1 of the boundary), so a `G` edit takes effect on the next deposit
without touching stored edge weights or requiring a runtime rebuild.

The diagonal is a per-cluster self-sustain macro — one knob for "how strongly
does the hat loop feed itself". Off-diagonals are the cross-cluster drive that
`H`-based handoff depends on (§4.6): **cross-inhibition with no cross-drive
cannot hand off**, and making that a visible zero cell in a 4×4 matrix is much
of the point.

`G` multiplies the *gathered* contribution, not the stored weight, so authored
weights and the delta overlay both stay meaningful and inspectable.

### 4.4 Activity traces

Each group carries a leaky trace of its recent firing rate. On each accepted
fire by node `i`:

```
activity[group(i)] += 1.0 / max(1, member_count[group(i)])
```

Normalizing by member count makes `H` entries independent of how many nodes
happen to be in a group — a 2-node group and a 9-node group firing "as much as
each other" produce comparable traces, so retuning `H` isn't required after
moving a node between groups.

The trace decays on the same finest-grid step as `apply_energy_decay`
(graph.rs:1676–1685 — the same tick that already drives `leak_deltas`), using
`group-trace-decay` interpreted per beat via `factor.powf(step_beats)`, the way
delta leak already converts. Clamp `activity` to `[0, 4]`.

Traces update **after** all fire decisions for a boundary are committed, so
within-boundary evaluation stays order-independent — the same double-buffer
discipline the deposit phase already documents.

### 4.5 `H` — activity coupling to threshold

Nodes read their threshold from a param (`:threshold` by convention, named by
`:group-threshold-param` so this generalizes). The engine adds a group-derived
offset to the resolved param value in `NodeEval.params` before invoking the
update closure (graph.rs:1458), so no node rule changes.

Three implementation constraints found in the current code:

- **The cached mirror must move too.** `nodes[idx].threshold` is cached out of
  the param map at graph.rs:2843–2844, and `apply_delta_key` already mirrors
  `"transpose"`/`"threshold"` writes into it (:1808–1812). Any group offset
  applied only to `NodeEval.params` would leave that mirror stale.
- **`propagation_selection_score` reads the cached threshold.** It grants a
  +1000 bonus when `energy[to] + amount >= threshold[to]` (graph.rs:2284–2301),
  so under `:propagation` selection the arbitration would rank on
  pre-suppression thresholds unless the mirror is kept current. Fixing the
  mirror fixes both.
- **Do not implement this on the delta store.** `GraphDeltaKey::NodeParam`
  would resolve correctly, but the delta layer leaks toward zero every decay
  tick (`leak_deltas`, graph.rs:1694–1707), is owned by processes, and
  `graph-commit-deltas!` folds whatever it finds into authored overrides —
  which would silently bake a transient suppression offset into the user's
  patch. The group offset is engine-owned and separate; it composes with
  deltas at resolution time and is never committed.

`NodeEval.params` is a full `HashMap` clone per node per boundary (:1458).
That is a pre-existing allocation seam, not one this feature introduces, but
since P3 touches this line it is the natural moment to consider hoisting the
threshold lookup out of the map.

```
suppression[g] = Σ_c  H[c][g] * activity[c]
θ_eff(i)       = clamp(θ(i) + suppression[group(i)] − rebound[group(i)],
                       0, θ_max)
```

- **Positive** `H[c][g]`: activity in `c` *suppresses* `g` (raises its
  threshold). This is the cross-inhibition primitive.
- **Negative** `H[c][g]`: activity in `c` *excites* `g`. Chains and cascades.
- **Diagonal** `H[g][g] > 0` is self-limiting — a group throttles itself as it
  gets busy, which is a per-group density governor and composes with (and is
  strictly simpler than) the global dynamic-threshold regulator sketched in
  §10.

Clamping at `0` on the low end matters: at `θ_eff = 0` the standard rule
`(>= (energy) (param :threshold))` is satisfied even at zero energy, so a
sufficiently excited group fires regardless of stored charge. That is the
guaranteed-wake-up lever, and §4.6 depends on it.

`θ_max` is the declared param range max (4 in the demo).

### 4.6 Debt, release, and rebound

This is the mechanism that makes a suppressed group come back. Two routes
exist and the design uses both, because they fail in different situations:

**Route 1 — integration during suppression (free, already true today).**
Suppression raises a node's threshold but never blocks its *inputs*. A
suppressed group keeps integrating whatever the dominant group sends it
(energy is retained sub-threshold and only zeroed on an actual fire), so it
charges like a capacitor while it waits. When suppression lifts, it discharges
onto energy already above threshold. Selective, musical — the node the
dominant group fed most is the one that leads the return — but it requires
`G[dominant][suppressed] > 0` and it can leak away over a long suppression.

**Route 2 — inhibition debt (new).** Modeled on post-inhibitory rebound in
real neurons, where the hyperpolarization *itself* creates the excitability.
Each group runs a slow follower of its own suppression:

```
debt[g]    += (suppression[g] − debt[g]) * group-debt-rate     // per step
rebound[g]  = max(0, debt[g] − suppression[g])
```

The follower lags. While suppression *rises*, `debt < suppression`, so
`rebound = 0`. While suppression *falls*, `debt` sits above it and `rebound`
goes positive by exactly the amount suppression dropped faster than the
follower could track. It decays to zero on its own as the follower catches up.

One state variable, no edge detection, no explicit derivative, self-arming and
self-terminating. And it feeds both release mechanisms:

- **Threshold undershoot**: subtracted in §4.5's `θ_eff`. With enough
  `group-rebound`, `θ_eff` hits `0` and the group fires even from zero energy.
  This is what guarantees the handoff when route 1 has nothing stored.
- **Energy surge** (the "opposite of decay"): during the same window, group
  members decay with an inflated factor:

  ```
  energy[i] *= clamp(decay * (1 + group-surge * rebound[group(i)]), 0, SURGE_MAX)
  ```

  Multiplicative on purpose. A zero-energy node stays zero, so surge cannot
  invent notes in a group nothing has touched; and because it scales stored
  charge, the *fullest* node crosses first — meaning the voice that leads the
  return is chosen by what the music actually did during the suppression,
  not by node index. `SURGE_MAX = 1.5` caps the per-step multiplier.

The division of labor: **undershoot guarantees the entrance, surge shapes the
voicing of it.** Set `group-surge` alone for organic handoffs that depend on
cross-drive; add `group-rebound` when the handoff must be guaranteed.

Per-group poly (§4.2) bounds the burst either way — a fully rebounded group
can't dump more than `max_poly` voices at once, and the losers land in §4.7.

### 4.7 `reject-retain`

`drop_firing` (`runtime/graph.rs:2158`) currently sets `energy = 0.0`. It
becomes:

```rust
self.energy[node_index] *= reject_retain(node_index);
```

Semantics by value:

- `0.0` — today's behavior; a rejected fire erases the node's charge.
- `0.5` (recommended) — the node comes out of rejection **sensitized**: above
  baseline, below threshold, needing only a small nudge from a neighbor to
  fire. Rejection becomes deferral rather than erasure, which is much closer
  to how a musician who got cut off behaves.
- `1.0` — full retention: the node re-fires at its very next quantize boundary
  and keeps retrying until it wins a slot. This is an emergent
  ratchet/roll — dense moments that overflow polyphony smear their losers into
  a fast retrigger tail. Possibly great on jungle material, possibly a mess;
  it is one knob away either direction, which is the argument for a param
  rather than a fixed policy.

### 4.8 Boundary evaluation order

The existing phases with the new work interleaved. Order matters for
determinism:

1. **Deposit** — scatter ready propagations into input accumulators, scaled by
   `G` (§4.3).
2. **Resolve group modulation** — compute `suppression[]` and `rebound[]` from
   the *current* (previous-boundary) `activity[]` and `debt[]`. Read-only
   snapshot.
3. **Fire decisions** — integrate input into energy, apply `θ_eff` into
   `NodeEval.params`, run the update closure, collect candidates.
4. **Arbitration** — per-group `max_poly_accept` (§4.2).
5. **Commit / drop** — accepted fires emit and propagate as today; rejected
   fires apply `reject-retain` (§4.7).
6. **Update group state** — add accepted fires into `activity[]`; step
   `debt[]`; then decay energy with the surge multiplier and decay `activity[]`.

Because step 2 reads a snapshot and step 6 writes, no node's decision depends
on another node's decision within the same boundary.

## 5. Lifecycle, determinism, serialization

- **Reset** (`:reset-every` boundary) clears `activity` and `debt` alongside
  `energy`, then re-seeds. This hard-resyncs the group dynamics to the bar
  grid, which is what keeps an emergent alternation period (§8) from drifting
  indefinitely.
- **Transport stop / pattern change / scene change**: same treatment as
  energy — group runtime state is ephemeral and rebuilt from zero.
- **Determinism**: group state is a pure function of the fire history since
  the last reset, so identical transport positions replay identically. This
  must hold under the lookahead/rebuild path the scheduler already exercises.
- **Serialization**: `:group` joins the per-node override struct; the two
  matrices and the six scalars join the graph config override struct. Both
  ride the existing per-pattern override serialization with a version bump.
  Defaults (§1) mean older projects deserialize to inert values.
- **Live update**: all group config must apply through the "update without
  replacing energy" path so edits during playback don't reset the graph.

## 6. UI

Additions to the demo panel (`graph-neural-variable-reset-demo.lisp`), all
following the existing `bind-graph` / `graph-key` / `reactive-set` pattern —
no shadow `defstate` per node:

- **`grp` column** in the per-node row: a 4-option dropdown (A–D), tinted with
  the group color. Slots next to `route`, and reuses the route-color-strip
  widget idiom for a group tint on the row.
- **Group matrices**: two `k×k` `matrix` widgets in the config panel — `G`
  (sequential fill, `0..2`) and `H` (diverging, `-2..2`, reusing the existing
  orange/blue negative-fill convention from the delta matrix). Both edit via
  `:on-cell-change` so a drag writes one cell.
- **Group state column**: a `group-count × 1` matrix showing `activity[]`, and
  a second showing `debt[]` — the diagnostic that turns "why did it stop" into
  "group C never accrued enough debt to escape". Rides the graph-visualization
  snapshot next to `:energy-matrix` / `:trigger-matrix`.
- **Scalars**: `trace decay`, `debt rate`, `rebound`, `surge` knobs in the
  config panel; `reject-retain` as a per-node column plus a global set-all
  (same treatment `threshold` gets today).

The node-count/group-count interaction: when `group-count = 1`, hide the
matrices and the `grp` column entirely so the panel is unchanged for users who
don't want this.

## 7. Implementation seams

Verified against the working tree; line numbers will drift.

| Piece | Where |
|---|---|
| Per-group runtime state | `GraphRuntime` SoA block, graph.rs:862–926 (next to `energy` :897); init `new_from_config` :1021–1034; clear `reset_internal` :1286–1324 |
| Per-group polyphony | `max_poly_accept` graph.rs:2196–2267 — partition candidates, reuse `accept_top_n` (:2270) per partition; `max_poly` itself is read off the runtime at `scheduler/lookahead.rs:1536` |
| `G` gain in gather | deposit phase, `process_block` graph.rs:1422–1430 |
| `θ_eff` injection | `NodeEval.params` build, graph.rs:1458 + cached mirror :2843–2844 (see §4.5) |
| `reject-retain` | `drop_firing` graph.rs:2158–2161 |
| Surge multiplier | `apply_energy_decay` graph.rs:1676–1685 |
| Trace/debt update | end of the boundary loop, after commit/drop (§4.8 step 6) |
| Node intrinsic override | `ProjectGraphNodeIntrinsicOverride` graph.rs:532–557 (mind the §3.1 collision); resolution in `runtime_config_with_overrides` :2755–2940 |
| Live update without reset | `apply_config_preserving_state` graph.rs:1099–1167 and `config_compatible` :1088–1097 — both need the new fields, or a group edit will rebuild and drop energy |
| Viz snapshot | `GraphVisualizationSnapshot` graph.rs:249–263, cloned in `visualization_snapshot_at` :1060–1086, surfaced in `graph_visualization_value` (`ui/state_values/topology_and_visualization.rs:352–503`) |
| Lisp natives | `lisp_host/eseq/graph_authoring.rs` — readers :128–182, setters :183–…, config :394–478 |

**Adding each config field is a six-point lockstep** (per the existing
`max-poly-selection` precedent), and missing any one of them fails silently:

1. `Option<T>` field on `ProjectGraphOverrides` (graph.rs:577–595) with
   `#[serde(default)]`;
2. read arm in `resolved_graph_config_value` (graph_authoring.rs:714–744);
3. write path: a `ConfigEdit` variant (:791–796), a parse arm (:798–817), and
   an apply arm inside `edit_current_graph_overrides` (:822–827);
4. field on `GraphRuntimeConfig` (graph.rs:788–805) + `new` (:808–851) +
   resolution in `runtime_config_with_overrides` (:2891–2911);
5. runtime field + copy in `apply_config_preserving_state` (:1114–1137);
6. for enum-valued fields, a `parse_*` plus display for
   `graph_config_display_value` (dropdown seeding).

The k×k matrices are the one field shape with no precedent here — everything
existing is a scalar `Option`. Storing them as a flat `Vec<f32>` of length
`k*k` with `#[serde(default)]` keeps the lockstep mechanical.

**Precedent worth mirroring**: track mute groups are the same shape of feature
one layer up — a small clamped integer id per entity
(`TrackParams.mute_group`, `sequencer/data.rs:856`, accessors :1072–1077,
clamped 0..=8), an options dropdown
(`build_mute_group_options`, `ui/state_values/param_fields_and_sync.rs:1450`),
and a winner-selection function at commit time
(`mute_group_winner_for_block_events`, `audio/events.rs:666–680`). Node groups
are that pattern applied to `max_poly_accept` instead.

## 8. Build phases

Ordered so each phase is independently useful and shippable:

| Phase | Content | Why here |
|---|---|---|
| **P1** | `:group` field + per-group polyphony (§4.1–4.2) + `reject-retain` (§4.7) + `grp` column | Smallest change that fixes the concrete observed pain. No new dynamics, no new state. |
| **P2** | `G` gain matrix (§4.3) + matrix widget | Pure macro over existing structure; still no new runtime state. |
| **P3** | Activity traces + `H` coupling (§4.4–4.5) + state columns | First new dynamics. Enables self-limiting diagonals and static cross-inhibition. |
| **P4** | Debt / rebound / surge (§4.6) | The handoff mechanism. Meaningless without P3. |
| **P5** | Tuning presets (§8) + scene morph of the k×k matrices | Turns the surface into performance controls. |

P1 should ship with a regression test asserting byte-identical emission for a
single-group graph, and a test that a group-B loop survives a group-A flood.

## 9. Tuning recipes

Worth shipping as documented starting points, because the interesting configs
are not obvious from the knobs.

### Half-center oscillator (two groups alternating)

Two groups, mutual positive `H` off-diagonals, plus non-zero `G` off-diagonals
so each charges the other, plus per-node `dampen`/`recover` inside each group
so the dominant one fatigues. Result: A dominates, fatigues, its suppression
of B weakens, B enters on stored charge and/or rebound, A recovers in silence,
roles swap. Alternation that was never sequenced.

### Escape vs release — the character knob

Half-center oscillators alternate through one of two mechanisms, and they
sound different:

- **Release**: the dominant group fatigues until it can no longer suppress,
  and the other is *let go*. Period governed by `dampen`/`recover`. Musically
  this breathes — a phrase plays out and hands off at a natural exhaustion
  point.
- **Escape**: the suppressed group's charge or debt builds until it crosses
  *while the dominant group is still strong*, and it *takes* the turn. Period
  governed by charge/debt rate. Musically urgent and interruptive — brief
  overlap, tighter cycles.

Which regime you get is set by the ratio of the dominant group's fatigue rate
to the suppressed group's charge rate (`G` off-diagonal + `group-debt-rate`).
One relationship spanning "conversational" to "argumentative" is the single
most expressive control this spec adds.

### Rotation with three or more groups

Asymmetric `H` (A suppresses B harder than B suppresses A, around the cycle)
produces cyclic dominance — a rotating wave A→B→C→A. The rotation *order* is
authored by the asymmetry pattern; the timing stays emergent. For drums that
is a kick/hat/perc turn-taking machine where you specify who follows whom and
the network decides when. Three-cycles are also structurally harder to lock up
than two, since there is always a next in line holding debt.

### Period and the grid

The alternation period is emergent — fatigue time plus recovery time — and
will not be a clean fraction of a bar. Fire times are still snapped by each
node's `quantize`, so the *events* land in time even though the underlying
oscillation is irrational, and `:reset-every` hard-resyncs the whole thing on
its boundary. Organic period, locked placement. Setting the reset window to a
multiple of the natural period versus deliberately against it is a
compositional choice.

### The three lockups to design against

- **Both silent** — mutual suppression too strong relative to drive. Debt-driven
  undershoot (§4.6) is the structural cure: silence accrues debt, so silence
  is self-terminating.
- **One group wins forever** — asymmetry too large, or `recover` outpaces
  `dampen` so the winner never fatigues. Immediately visible on the activity
  columns.
- **Flutter** — alternation every boundary instead of every phrase;
  suppression too weak or fatigue too fast. The trace decay constant is the
  only hysteresis in v1; see §9.

## 10. Open questions

- **Flutter guard.** Is `group-trace-decay` enough hysteresis, or does a group
  that just took over need an explicit minimum dwell before it can be
  re-suppressed? Deliberately left out of v1 to keep the state small; revisit
  after hearing P4.
- **Global voice ceiling.** `group_count × max_poly` total voices is intended,
  but should there be an optional global cap applied after per-group
  selection, for users who care about absolute density?
- **Should `G` scale edge `dampening` too**, or only `weight`? Scaling only
  weight means a group turned down by `G` still fatigues at the same rate.
- **Velocity-weighted activity.** Should the trace increment by fire velocity
  rather than a flat 1.0, so quiet ghost notes count less?
- **Per-node debt.** Debt is per-group here. Per-node would let individual
  nodes rebound independently, at 16× the state and a much less legible UI.
- **`group-threshold-param` generality.** Pointing `H` at `:vel-decay` instead
  of `:threshold` gives "when A is busy, B plays quieter" for free. Worth
  exposing in the UI, or leave as a manifest-level choice?

## 11. Related and deferred

- **Dynamic threshold / density regulator.** A global loop that nudges
  threshold to hold a target fires-per-bar, with integral control, asymmetric
  clamps (freely downward, barely upward), a fast fizzle reflex, and
  measurement taken on *candidate* fires rather than post-arbitration fires
  (otherwise `max-poly` saturation blinds the controller). It composes with
  this spec: the regulator holds global energy, `H` decides who spends it.
  Note that the `H` diagonal is a per-group version of the same idea, so
  building P3 first may reduce how much the global regulator needs to do.
- **Ring visualization.** The layered feedforward NN diagram is the wrong
  shape for a recurrent delay network. A chord/ring layout — nodes on a circle,
  edges as interior arcs, node fill = `energy/θ_eff` readiness, and *pulses
  animated along arcs* positioned by `elapsed/delay` from the pending-arrival
  queue — would show causality in motion, which no matrix view can. Groups
  give it its coloring and a natural cluster-arc summary layer. Wants to be a
  builtin widget fed by the graph-viz snapshot, not lisp-composed.
- **Low-rank latent control.** The full hidden-layer version (`θ = θ_base +
  U·h`, `h = decay(V·recent_fires)`) generalizes `H`; user-defined groups are
  the interpretable special case where the factorization is authored rather
  than learned. A cheaper relative with the same payoff: a 2D morph pad
  interpolating between four saved graph configurations.
- **Retained-energy-on-rejection as a global default.** If `reject-retain =
  0.5` proves universally better in practice, consider flipping the default in
  a future project version rather than leaving it opt-in.
