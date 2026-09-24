# Graph RNG: seeds, reloads and generator character

Status: spec rev 1, 2026-09-24. Not built. Host UI target:
`alez.neural.variable-reset`. Related: `docs/graph-node-processes-spec.md`
(node process chains, `graph-reset!`, per-fire process seeds from eseq-waa9.7).

## 1. Problem

A neural sequencer in `markov` or `random` mode makes a different phrase after
every reset. Often that's what you want, but you can't do anything else:

- You can't say "replay the phrase you just played" (reload the RNG on reset).
- You can't say "cycle through 4 phrase variants" or "pick phrase family #17".
- You can't change how random it is: fully random vs. a short, audibly
  repeating pseudo-random loop (an LFSR, or a Turing Machine register with a
  lock knob).
- A neural process can't touch the RNG, so reseeding can't be sequenced.

The randomness should be something you play with, not a hidden constant.

## 2. What exists today

Two independent streams, neither controllable:

1. **Graph stream.** `GraphRuntime.random_state: u64`
   (`runtime/graph.rs`), a splitmix64 counter seeded from `config.id`.
   `next_random_u64` feeds:
   - the markov weighted edge choice (`next_random_unit() * total` over the
     firing node's out-edge amounts), and
   - `weighted_sample_without_replacement` for `random` / `markov` max-poly
     selection.
   No reset path (`reset_internal`, `reset_group`, `reset_node_state`) touches
   it, so each phrase continues one endless stream.
2. **Node-process stream.** Every node patch slot is seeded by
   `process_rng_seed(instance, policy, Step{cycle, step})` and then
   `mix_fire_seed(seed, ctx.fire_seed = sample_time)`
   (`runtime/process.rs`, `scheduler/node_process.rs`). eseq-waa9.7 added
   that so a `:locked` class doesn't roll the same value on every fire. The side
   effect is that a node's `rand` card never repeats either.

## 3. Design

### 3.1 Streams are per neural group

Resets are already scoped by group (`reset_group`, `graph-reset! group`), so
the RNG is too. Otherwise reloading group A would shift group B's choices.

```
GraphRuntime {
    rng: [GraphRng; NEURAL_GROUP_MAX],   // one per group (NEURAL_GROUP_MAX = 4)
    arbiter_rng: GraphRng,               // max-poly selection (graph-wide)
}
```

- A markov edge choice draws from the **firing node's group** stream.
- Max-poly selection ranks candidates from every group together, so it draws
  from `arbiter_rng`. A whole-graph reset reloads it; a group reset doesn't.
  (An arbiter lock with no group-level lock is a legitimate setting: same
  voice-stealing each phrase, free paths.)

With the default policy (§3.2 `free`, `splitmix`, seed derived from
`config.id`), draws are **bit-identical to today** for single-group graphs.
Group 0 reuses today's state and consumption order. The existing
seeded-selection tests (`chooses_first.random_state = 3`, the `seed` loop near
`graph.rs:4788`, the `random_state == 123` round-trip) pin this. Multi-group
graphs change stream assignment; that's accepted, since nothing persists RNG
state.

### 3.2 Seed + reload policy (sequencer-level config)

New `ProjectGraphSequencerOverride` fields, like `reset_every_beats` /
`max_poly` (None = inherit the manifest default):

| field | type | meaning |
|---|---|---|
| `rng_seed` | `Option<[u64; 4]>` per group, plus arbiter | base seed; None = derived from `config.id` + group |
| `rng_reload` | `Option<[RngReload; 4]>` | when the stream returns to its base seed |
| `rng_reload_arbiter` | `Option<RngReload>` | same, for max-poly selection |

```rust
enum RngReload {
    Free,            // never (today)
    OnReset,         // every reset of this group: identical phrase each time
    Cycle(u16),      // every k-th reset: k distinct phrases, then repeat
}
```

`Cycle(k)` keeps a per-group `resets_since_reload` counter. On each reset of
the group: if the counter hits k, reload the base seed and zero it. Otherwise
leave the stream running. `Cycle(1)` is `OnReset`.

**Reload** means setting the generator state back to its base seed, not just
reseeding. A `turing` register (§3.4) gets its initial bit pattern back.

Reload happens inside `reset_node_state`'s callers (`reset_internal` reloads
every group plus the arbiter, `reset_group(g)` reloads g). The
transport-driven reset (`reset_interval_beats` boundary) and fire-authored
resets (`reset_clearing_pending`, `requested_resets`) go through the same path,
so every kind of reset honours the policy.

Lisp:

```lisp
(graph-config self :rng-reload :on-reset)           ; all groups
(graph-config self :rng-reload '(:on-reset :free :free :free))
(graph-config self :rng-reload-cycle 4)             ; Cycle(4)
(graph-config self :rng-seed 17)
(graph-config-value self :rng-seed)
```

### 3.3 Driving the RNG from a process

A graph-control command with the same lifecycle as `graph-reset!`: the node
runner collects it into `NodePatchOutcome`, and it's applied after the
boundary's commits. That way a mid-boundary reseed never splits one
boundary's draws across two streams.

```rust
GraphControlCommand::Rng { group: Option<u8>, op: RngOp }
enum RngOp {
    Reload,                 // back to the base seed (does not change it)
    Seed(u64),              // set base seed AND reload
    Advance(u32),           // burn n draws: nudge to a neighbouring phrase
    Flip(f32),              // turing only: set flip probability p (§3.4)
}
```

Natives (process bodies and UI):

```lisp
(graph-rng-reload! group)        ; group nil = every group + arbiter
(graph-rng-seed! group n)
(graph-rng-advance! group n)
(graph-rng-flip! group p)
```

Track processes reach a graph the same way `graph-reset!` does from a track
(`reset_now` path, landing at `last_boundary_beats`).

**Builtin card `neural-seed`** (node flavoured, shown in the node bay):

| inlet | kind | meaning |
|---|---|---|
| `gate` | gate | only act when high |
| `op` | enum `reload / seed / advance / flip` | |
| `value` | scalar | seed (rounded to int), advance count, or flip p |
| `group` | enum `own / all / A..D` | `own` = the node's group |

This composes with what's there: `rand -> neural-seed.value` picks a new
phrase family per hit; `cmp(reset-fired?) -> neural-seed.gate` with
`op reload` reloads only on a node-authored reset; a slow `count` into `value`
walks seeds 0..n.

### 3.4 Generator character

`GraphRng` is an enum. It's chosen per group with an `rng_kind` override,
default `splitmix`:

| kind | state | character | params |
|---|---|---|---|
| `splitmix` | u64 counter | white; never audibly repeats | none |
| `lfsr` | L-bit Galois LFSR, maximal taps table for L = 3..32 | periodic, period 2^L − 1 draws; short L is a "weird repeating" sequence | `length` |
| `turing` | L-bit shift register | Music Thing Turing Machine: each draw rotates the register and flips the recycled bit with probability p | `length`, `flip` |

`turing` gives the lock-knob range: p = 0 is a locked loop of L draws,
p ≈ 0.5 is fully random, p = 1 is an inverted loop of period 2L. `flip`
is the knob to automate (`graph-rng-flip!`, `neural-seed op flip`). The
flip coin itself comes from a private splitmix sidecar, so `p` never affects
the register's own sequence when no flip happens.

Output: all kinds expose `next_u64`. `next_random_unit` stays
`(u >> 11) / 2^53`. For the short-state kinds, the draw is the register
widened through `splitmix64(register)`. Then a 4-bit LFSR yields 15 distinct
*uniform* values rather than 15 tiny ones clustered near 0. Periodicity is
the character; bias isn't.

**Draws aren't beats.** A loop of L draws doesn't line up with bars: how many
draws a phrase uses depends on how many candidates compete at each choice, and
markov only draws when a node has more than one live out-edge. So `turing` at
p = 0 gives a repeating *sequence of choices*, not a repeating bar loop, and
the two drift unless the graph's topology keeps draw counts constant. For
bar-locked repetition, use `rng_reload: on-reset`. The two combine:
`turing` with `on-reset` repeats bar-aligned, and `flip` mutates the next
phrase.

### 3.5 Node-process dice follow the group stream

Replace the per-fire `sample_time` in `mix_fire_seed` with a value from the
group's policy:

```
fire_seed = hash(group_base_seed, group_reload_epoch, fires_since_reload[node], node)
```

- `group_reload_epoch` increments on every reload in `Free` mode (every reset
  counts). In `OnReset` / `Cycle` it holds between reloads.
- `fires_since_reload[node]` zeroes on reload.

Result: with `free` the node's `rand` card rolls fresh on every fire (same
character as eseq-waa9.7, no longer tied to wall-clock samples). With
`on-reset`, the dice replay each phrase exactly like the graph's choices.
Process dice don't *consume* the graph stream, so adding a `rand` card never
changes the graph's path.

### 3.6 What "identical phrase" promises

Reload guarantees the **same dice**, not the same notes. The path matches
only if the inputs match too:

- route-seeded nodes (`:seed-from :route`) listen to live tracks,
- max-poly competition depends on what else is sounding,
- edits between phrases (transpose, dampening, edges) change weights.

That's the intended use: same dice, different input. Document it in the
panel tooltip.

### 3.7 Lifecycle gotchas to verify when building

- **Runtime rebuild.** `reconcile_graph_runtimes` may rebuild a
  `GraphRuntime` on edits, which currently reseeds `random_state` from
  `config.id`. With `free` that is a silent phrase jump on every knob edit;
  carry `rng` state across rebuilds when the topology is compatible (as
  energy is carried, if it is), otherwise reload.
- **Lookahead.** Confirm draws aren't consumed by speculative scheduling
  that later gets discarded. If they are, a locked phrase would drift under
  transport changes.

## 4. UI (alez.neural.variable-reset)

Top panel, next to `reset bars` / `max-poly`:

```
rng [splitmix ▾]  reload [on-reset ▾] (k 4)  seed [ 17 ]  ⟳
```

- `⟳` = `graph-rng-reload! nil`.
- With `turing` / `lfsr` selected: `len [ 8 ]` and (turing) a `flip` knob.
- Per-group overrides live in the groups panel (one row per group: kind,
  reload, seed). The top row edits "all groups".
- A small indicator per group: `resets_since_reload / k` in `Cycle` mode,
  so you can see where you are in the variant cycle.

## 5. Phases (beads, epic TBD)

1. **Per-group streams + reload policy.** `GraphRng::Splitmix` only;
   `rng_seed` / `rng_reload` overrides + natives + serde; reload in the
   reset paths; bit-identity tests for the default; `OnReset` replays a
   markov phrase exactly (scheduler harness, route-free graph); `Cycle(3)`
   gives 3 distinct phrases then repeats; group reset reloads only its group.
2. **Process control.** `GraphControlCommand::Rng`, the four
   `graph-rng-*!` natives, the `neural-seed` builtin; test a node-authored
   reseed applies after the boundary's commits.
3. **Node-process dice follow the policy** (§3.5). Replaces the
   `sample_time` fire seed; waa9.7's "rolls fresh per fire" test still passes
   under `free`.
4. **Generator kinds.** `lfsr` + `turing` with `flip`; period tests
   (LFSR period 2^L − 1, turing p=0 period L, p=1 period 2L).
5. **UI.** Top-row controls, per-group rows, cycle indicator; capture
   fixture.
6. **Rebuild carry-over** (§3.7), unless phase 1 finds rebuilds are rare
   enough to fold in there.

## 6. Not doing

- Seeding from audio or external entropy. `neural-seed.value` can already
  be wired from anything a process reads.
- Per-node streams. Per-group is the granularity resets already have; a
  per-node lock is `neural-seed` with `group own` on a single-member group.
- Persisting the running RNG state in the project. Reload-to-seed is
  the reproducibility story; save/load restarts from the base seed.
