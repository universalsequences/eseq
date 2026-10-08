# Jaki Trig Modes — neural (and other) sequencers play jaki

> Names below predate kind bindings (eseq-0l17); see docs/kind-bindings-spec.md.

Status: rev 2, 2026-09-29 — §2-§6 BUILT (eseq-jtrg.1-.5). Epic `eseq-jtrg`;
§7 is `eseq-jtrg.6`.

## 1. Idea

Neural decides *when*, jaki decides *what*. A neuron routed to a jaki
instance does not sound a note: each fire **gates** the jaki pattern open for
the fire's duration, and the fire's note and velocity shape what jaki plays in
that window. Jaki's own clock stops being the transport and becomes a
**playhead that advances only while gated**.

```
neuron fire  dur=3 note=+5 vel=0.5          dur=5 note=0 vel=1
jaki (. - . . . -)  in :continue            ─────────────────
plays                .  -                    .  .  .  -
                     (+5 st, ×0.5 vel)       (as written)
```

## 2. Modes (jaki instance document `mode`)

| mode        | clock                          | a gate fire                          |
|-------------|--------------------------------|--------------------------------------|
| `:loop`     | transport (`gen-tick`), today  | ignored (restart still applies, §5)  |
| `:retrig`   | virtual playhead               | playhead → 0, then plays `dur`       |
| `:continue` | virtual playhead               | plays the next `dur` units from where it stopped |
| `:gate`     | transport                      | unmutes the free-running pattern for `dur` |

`:loop` is the default, so every existing instance is unchanged.

In `:retrig`/`:continue` the virtual playhead advances one unit per jaki tick
*while the gate is open* and holds while it is closed. Everything downstream
of the position — `locate`, cycle index, `(every …)`, `(cyc …)`, the
hand/velocity threading, seq counters — sees the virtual position, so the
pattern **evolves only while it is played**. A retrig's jump back to 0 is a
backward cycle jump, which `ensure-state` already treats as "restart threading
from defaults": a retrig starts the phrase fresh, hands and all.

## 3. Payload pass-through

A fire's payload is latched when it is delivered and applies to every hit
jaki emits while that gate is open:

- **note** — the fire's emitted `:note` (neural: `global-transpose + in-note +
  transpose`, as it would have played on a track) is **added** after
  everything jaki computes: `note = (row note-set or instance note) + row
  note-adds + fire.note`. It is added *after* an absolute `(note …)` set, so a
  row `(note 12 18 32)` under a fire of +5 plays 17 23 37.
- **velocity** — **multiplied**: `vel = jaki model vel × row/instance scale ×
  fire.vel`. Accent contour survives; the fire sets the level.

`:loop` ignores payload (gain 1, +0). `:gate` applies it.

## 4. Gate semantics (engine)

A gate trigger is `{beat, duration_beats, note, velocity}`, `beat` = the
fire's straight grid beat (after `:quantize`, before swing).

- Delivered at the first jaki boundary `b ≥ beat − ε`. Delivery bumps the
  generator's gate **epoch**, sets `open_until = max(open_until, beat +
  duration_beats)`, and latches `note`/`velocity`.
- **Overlap:** the window only ever extends (order-independent for coincident
  fires). Payload is newest-wins across boundaries; among fires delivered at
  the same boundary the loudest wins (deterministic).
- `open` at boundary `b` ⇔ `b < open_until − ε`. So a fire of 3 units at a
  jaki boundary plays exactly units 0,1,2 of its window.
- Units, not steps: durations cross in beats. A neuron at `:16` with
  `dur=3 steps` gates 0.75 beats = 3 units of a `:16` jaki, 6 units of `:32`.
- Sub-unit hits (a dash's second hit, ghost pickups) belong to the unit that
  owns them, which is gated at its boundary; gate tails ring past the close.
- Transport reset (stop/play, song wrap) clears gate state and epochs with
  the rest of the generator runtime.

The tick reads it with `(gen-gate)` →
`(dict :epoch n :open bool :note st :vel v :restart n)`; never-triggered is
`epoch 0, open false, note 0, vel 1`.

### 4.1 Same-boundary ordering

Within one scheduler chunk the order is generators → processes → graphs, and
a chunk is one audio block (shorter than a step). A fire at beat B would reach
a generator whose tick for B already ran. Fix: **gated generators run in a
second pass after the graph stage.** A generator is gated when any graph node
in the chunk routes to it. Pass 1 skips it; pass 2 runs it over the same chunk
after graph fires have queued their triggers. Ungated generators keep today's
position, so graph `:output` reads of them are unaffected.

Consequence: a gated jaki's hits are not visible to graph `:output` reads in
the same chunk (they are one chunk later). A graph that both drives a jaki and
reads that jaki's track is a feedback loop; one chunk of latency on the read
side is acceptable.

## 5. Restart route (`eseq-jtrg.5`)

A second route option per jaki: **"↺ <jaki>"**. A fire routed there bumps
only the generator's **restart epoch** — no gate, no payload. The jaki clock
reacts in every mode:

- `:loop` / `:gate` — re-anchor: position = `gen-tick − anchor`, anchor = the
  tick the restart landed on (subsumes `eseq-jo7.6`).
- `:retrig` / `:continue` — playhead → 0 without opening the gate.

So a neuron can reset a freely looping jaki to the top of its phrase, or pull
a `:continue` jaki back to the start of its song.

## 6. Routing surface

- Engine: `ProjectGraphRouteOverride::Generator(id)` /
  `GeneratorRestart(id)` (serialized `{"generator": id}` /
  `{"generator_restart": id}`), resolved onto `GraphNode.gate_target`
  (`GateTarget { id, restart }`); the node's track route is `None`. A gate-target fire never enqueues a note and is not
  retained for resync replay (the generator consumed it; generator state is
  not rewound on resync). Seed-from `:route` on a gate-target node seeds
  nothing.
- Lisp: `(graph-node self n :route (list :gen id))` or `(list :restart id)`;
  `(graph-node-value self n :route)` reads the same form back.
- Neural panel (`alez.neural` variable-reset): route dropdown lists tracks,
  then every jaki instance as `→ <label>` (gate), then as `↺ <label>`
  (restart), then Off. Only jaki instances with the neural instance's owner
  (project, or the same rack) are listed. Track-typed process inlets keep
  their tracks-only list.
- Jaki panel: a `mode` dropdown (loop / retrig / continue / gate). The hit
  strip's playhead follows the virtual position and goes dark while gated
  closed.

### 6.1 Jaki core surface

- `alez.jaki.core/step-clock mode` sets the tick's position and the gate's
  note/vel pass-through (state cells `jaki-pos`, `jaki-gnote`, `jaki-gvel`)
  and returns whether the tick plays; `play-routes body` plays at that
  position; `run-in mode body` is both; `run body` = `run-in :loop`.
- `play-pos` is the clock's position only when `step-clock` ran on this
  tick; a hand-written `def-sequencer` that calls `emit` directly plays at
  `gen-tick` exactly as before.
- Known gap: a `:retrig` rewind inside cycle 0 (window shorter than a cycle)
  is not a cycle jump, so `(seq :hit …)` counters keep counting across fires.

## 7. Out of scope (follow-ups)

- `jak` surface form `:mode` (non-instance generators have no route id).
- Gate sources other than graphs (MIDI notes, processes, other generators).
  The trigger struct is source-agnostic; only the graph stage pushes today.
- Per-row modes.
