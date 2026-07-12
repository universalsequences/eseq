# Step Processes: Trajectory to the End Game

Status: planning side-doc, 2026-07-10. Companion to
`docs/cirklon-process-accumulator-brainstorm.md` (the normative spec — locked
decisions and phase details live there, not here). This doc answers one
question: **what is the shortest path from what has landed to the band-model
end game**, deliberately deferring presets and UI polish.

## Where we are

Landed and verified in code (through `main` at merge commit `f9f10cea`):

| Phase | What it delivered |
|---|---|
| 0–2 | Accumulators end-to-end, `(processes :track ...)` chain attachment, lane-backed inlets, UI lanes + slot editor. `def-accumulator` sugar (an early Phase 8 slice). |
| 3A/3B | Ports vs. bindings, hint resolution (`step-param` / `param-tag` / `midi-fx-target`), transient fire-time overlays, arm-to-map mapping UI, stale/unbound badges. |
| 4 | `veto!`, `ratchet!` (both modes), process-inlet ports + `connect!` + `process-inlet`/`inlet`/`wire` constructor natives, `dice` / `prob-mask` / `repeater` library entries. |
| 5 | Process chain in the fx panel. |
| 5B | Project process layer: `(processes :project ...)`, project-before-track composition, per-track runtime state/RNG, persistence, and PROJECT-badged fx-panel rows (items 1–3 + 6). |
| 5C | Per-track copy-on-write lane overrides for project slots, including persistence and reverting to the shared project lane. |
| 7 | Previous-tick resolved track reads, 256-entry step/trigger histories, process state/outlet and channel reads, `echo-track` / `wrap-crash` library proofs, and a runnable Phase 7 reads demo. |
| Band slice 2 | Typed `pitch-field` / scalar / gate suggestions, previous-tick nil-safe `hear`, `follow-harmony`, and a three-track band demo. |
| Band slice 3 | One-instance `:observe` / `:play` conductor attachment, post-resolution coalesced invocation, bound-track emissions, and a four-track conductor demo. |
| Player slice 1 | Self-contained conductor demo with phrase note count, integer delay, sequencer timebase spacing, and density-driven distribution across played tracks. |

Not landed: Phase 3C (rack write application), the remaining Phase 5B shared-
brain work (latched inlet wires and `target-mul!`), Phase 6 (presets), Phase 8
(remaining sugar + curve preview), Phase 9 (timebase).

## The end game, restated

The spec's "End Game" section: a library of **players, not just utilities**.
A pack like "es chord 2" loads from the browser, observes melody tracks, plays
harmony tracks toward a goal density — while the richer, decentralized version
is the **band model**: processes `suggest` typed fields (pitch-set, density,
accent) that any track opts into hearing on its own terms. Command spectrum:
`:play` (the band's hands) · `:steer` (lean on a player) · `suggest` (the vibe).

The spec names four engine deltas for this. Only three engine slices actually
gate the music; the rest is polish.

## The critical path: three slices

The ordering is forced by one shared mechanism: the **previous-tick register
rule**. Both cross-track grabs (Phase 7) and field `hear` reads use the same
determinism contract — reads see resolved state as of the end of the previous
step, never same-tick. Build the registers once; both features ride them.

### Slice 1 — Phase 7 reads (landed foundation)

- `read` expression family: another track's current resolved param value,
  `:steps-ago n` (time-locked history — canons), `:trigs-ago n` (event-locked
  — call-and-response), process outlet/state, channel value.
- Per-track **resolved-value registers** (post p-lock, post process writes),
  updated on trig fire; history buffers record register state at step
  boundaries. Gaps never produce nil; pre-first-fire reads return the base
  value (defaults-inert).
- Registers/history follow the accumulator reset policy (clear on pattern
  change by default).
- Library proof: `echo-track` and `wrap-crash` from the spec's examples.

Why first: conductors need harmonic context from observed tracks, and fields
need the register determinism rule. Everything downstream sits on this.

### Slice 2 — Fields: `suggest` / `hear` (landed)

A small delta once registers exist, because channels already exist:

- `suggest` publishes a **typed field value** on a channel — `pitch-field`,
  scalar, gate; the domain rides with the value.
- `hear` reads the field **as published at the end of the previous tick**
  (register rule again) and is nil-safe: no publisher ⇒ follow processes are
  inert. Defaults-inert holds automatically.
- Acceptance/obedience is an ordinary chain process in the listening track's
  own chain (`follow-harmony` with a lane-sequenceable `amount` inlet) — no
  new attachment machinery at all.
- Collisions: field names are plain channel names; two publishers on one field
  are the author's problem (decided).

This slice alone delivers most of the band model: any process can suggest,
joining the band is "drop `follow-harmony` on the track", listening is a mesh.
No conductor needed yet.

### Slice 3 — Conductor attachment mode (landed)

The one genuinely new invocation shape:

- `(processes :observe (list 1 2) :play (list 3 4 5 6) (es-chord-2 ...))` —
  one instance bound to N observed + M played tracks.
- Invoked **once per tick, after all observed tracks resolve**. This is the
  designed answer to same-tick cross-track ordering: post-resolution
  invocation of a single instance, no general dependency graph.
- It *plays* its tracks through **emissions** — the existing
  `EmittedAccumulatorEvent` → `enqueue_due_process_emissions` path, which is
  already track-agnostic and MIDI-FX-aware. It never edits step data.
- Its inlets stay ordinary inlets, so other tracks' process lanes can
  sequence the conductor (`ProcessInlet` targets already exist from Phase 4).

Start conductors as **raw `def-process`** — the best-effort determinism tier
is already sanctioned. The `def-conductor` pure-function tier (seed + resolved
reads + inlets) is added only if replay problems bite in practice.

After slice 3, the first "es chord 2"-style pack is authorable as plain Lisp:
observe two melody tracks, `read` their recent pitches, emit harmony toward a
density goal, `suggest :harmony` for anyone else listening.

## Deliberately skipped or deferred

| Item | Why it doesn't gate the end game |
|---|---|
| Remaining Phase 5B (shared/self-clocked brains, latched wires, `target-mul!`) | The project layer and per-track lane overrides have landed. These remaining items gate nothing in Phases 6–9; fields cover the energy-brain musical territory through channels. Pick them up later for the "DAW extension" surface. |
| Phase 6 (presets, tiers 1–3) | Packaging polish. Packs ship as scripts until then. |
| Phase 8 (rest of sugar + curve preview) | UI/ergonomics. Raw `def-process` reaches everything. |
| Phase 3C (rack writes) | Independent follow-up; rack targets stay soft no-ops. |
| Phase 9 (timebase) | Riskiest, gates nothing, explicitly last. |
| Reactive outlet → widget bindings | "See the conductor's state in a panel" polish; the music works without it. Promote when pack UIs get built. |
| `def-conductor` determinism tier | Raw tier suffices to start; add on evidence. |

## Doors to keep open while building the slices

From the spec's end-game section — cheap now, expensive to retrofit:

- Keep the emission path fully track-agnostic (it already is; don't regress).
- Keep `ProcessInlet` a first-class `ParamTarget` (the "sequence the
  conductor" seam).
- Don't let chain-slot identity/storage hard-assume one-instance-per-track —
  a conductor is an N-track instance.
- Registers/history retain exactly 256 step boundaries and 256 fired triggers
  per track. Keep conductor read patterns within that explicit window.

## Sequence, in one line

Expand the first player pack beyond the landed call/response voice. Presets,
sugar, previews, and panels come after the band can play.
