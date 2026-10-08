# Jaki P-locks — per-hit parameter sequences with track-aware completion

> Names below predate kind bindings (eseq-0l17); see docs/kind-bindings-spec.md.

Status: rev 1, 2026-09-30 — design only, nothing built. Epic `eseq-jplk`
(children listed in §10).

## 1. Idea

Jaki already sequences pitch per hit: `(note (seq :hit 0 5 9 12))`. The same
move for **any lockable parameter** of the track a row plays:

```lisp
(plock "instrument:cutoff" (seq :hit 100 5535 34 535))
(plock "fx2:filterbank:freq" (0.2 0.8))          ; implicit cyc, one per cycle
(on left (plock "rack-macro:macro_1" 0.9))       ; scoped, like (on SEL (note …))
```

Each value rides on the hit it lands with, exactly like a step p-lock: it
applies to that note only, and the next hit without a `plock` plays the step's
own params. In the sexp editor, the name argument is a string, and typing in it
pops a completion list of the **parameters of the track this row is routed
to**, fetched lazily from the host. The schema carries an identifier, never
the list.

## 2. Parameter names: one namespace, the macro editor's

`plock` names use the labels `process_param_target_label`
(`crates/sequencer/src/ui/state_values/process_and_macros.rs:78`) already
shows in the macro editor, so a name reads the same everywhere:

| target                         | label                        |
|--------------------------------|------------------------------|
| instrument param               | `instrument:cutoff`          |
| effect param (slot 2)          | `fx2:filterbank:freq`        |
| MIDI FX param (slot 1)         | `midi-fx1:arp:rate`          |
| step param                     | `step-param:chop`            |
| rack slot param                | `rack3:gain`                 |
| rack slot instrument param     | `rack3:instrument:cutoff`    |
| rack macro                     | `rack-macro:macro_1`         |
| bus send                       | `send:<bus>`                 |

`param` is the DSP param name or tag (`has_tag_or_name`), not the knob's
display label; `effect` is the effect descriptor name, case-insensitive.

**Slot-free spellings** (from the `ProcessTargetHint` labels, same file ~l.60)
are also accepted: `effect-param:filterbank:freq` matches the first Filterbank
in the destination's chain and `midi-fx-param:arp:rate` the first Arp. They
survive chain reorders and match across tracks whose chains differ. Completion
offers the slotted form (§5); the slot-free form is for people who type it.

Process inlets (`process:…`) are not per-hit parameters and are rejected.

### 2.1 Parsing is new

Today the labels are display-only: nothing parses a label back into a
`ParamTarget`. This spec adds `ParamRef::parse(&str)` (sequencer crate, next
to the label function) producing an **unresolved** reference:

```rust
enum ParamRef {
    Instrument { param: String },
    Effect { slot: Option<usize>, effect: String, param: String },
    MidiFx { slot: Option<usize>, fx: String, param: String },
    Step { param: StepParam },
    RackSlot { slot: usize, param: String },
    RackSlotInstrument { slot: usize, param: String },
    RackMacro { macro_idx: usize },
    Send { bus: BusId },
}
```

It stays unresolved because the destination track is not known when the
pattern is written, and may change (§6). A round-trip test pins
`parse(process_param_target_label(t))` for every `ParamTarget` variant except
`ProcessInlet`, so the display format and the parser cannot drift apart.

## 3. Engine: `seq-emit :params`

`seq-emit` gains `:params`, a flat list of label/value pairs:

```lisp
(seq-emit :track 3 :at 0.25 :note 0 :vel 1 :dur 0.25
          :params (list "instrument:cutoff" 5535 "fx2:filterbank:freq" 0.8))
```

- `build_seq_emit_event` parses each label once (`ParamRef::parse`); an
  unparseable label is a tick error (loud, like other `seq-emit` misuse).
  Parsed refs ride on `EmittedAccumulatorEvent` in a new
  `named_params: Vec<(ParamRef, f32)>`. `effect_params` / `instrument_params`
  stay empty at emit, as now.
- **Resolution at landing.** In the lookahead, where generator emissions get
  `StepLanding::resolve(...)` and the step-in-force params are stamped
  (`scheduler/params.rs:60-70`), each `named_params` entry is then resolved
  against the **destination track** in the chunk snapshot and folded in with
  `upsert_instrument_params` / `upsert_effect_params`. Stamping first and
  upserting second means a jaki `plock` beats the step's base value, its stored
  p-lock, and the print latch for that one hit: it is the most specific source.
  Step params (`step-param:*`) write the event's `ResolvedStep` fields.
- **Unmatched refs are skipped silently.** A name that does not exist on the
  destination (§6) drops that one param; the note still plays. No tick error:
  rerouting mid-play must never break a pattern.
- Resolution is by name, so it is cached per (track, label) and invalidated
  on the snapshot's descriptor epoch. Rack-owned generators resolve against
  the member track after `map_rack_member_emissions`, so a pad resolves
  against its own instrument.

**Values** are in the parameter's own units (what the knob and the tracker
cell show), clamped to its range at resolution. `automation_scale` already
knows each target's range and unit; the resolver uses the same data.

## 4. Jaki surface: `(plock NAME V)`

A per-hit word in `alez.jaki.core`, shaped like `note`:

- `V` is anything a per-hit value accepts: a number, an implicit-cyc list,
  `(seq :hit|:fig|:span|:cycle …)`, `(cyc …)`. Clocked args lower to
  `(:defer :plock raw key)` exactly as `note` does (spec §7.2 in
  `crates/sequencer/docs/jaki-sequencer-spec.md`), with the label folded into
  the defer key so two `plock`s on one row keep separate `seq` counters.
- `apply-defer` gains `:plock`, accumulating into the event's `:params` dict
  keyed by label; a later `plock` of the same label on the same hit wins.
- `emit-one` flattens `:params` into `seq-emit :params`.
- Works at row level and inside `(on SEL …)`, so `(on left (plock …))` locks
  only the left-hand hits.
- `NAME` is a string at runtime. Its only runtime check is `ParamRef::parse`
  at emit time; existence is checked at landing (§3) and in the editor (§6).

`fig-len` and the length-only evaluator never see `plock` (it is not
length-changing), so the lens memo is unaffected.

## 5. Editor: lazy schema, host-resolved completions

The slot schema today is a static grammar, so baking a track's parameter list
into it would mean shipping hundreds of words per row and rebuilding the
schema on every route or chain change. Instead:

### 5.1 `(dyn SOURCE)` schema atom (eseqlisp, generic)

```lisp
(form plock (fixed (dyn param)) (num :min -100000 :max 100000))
```

- `(dyn SOURCE)` is a string atom whose completions and validity come from a
  **host-registered source** named `SOURCE`. The widget stays ignorant of
  tracks and parameters.
- The widget takes a new prop `:dyn-context` (any Lisp value; here the row's
  destination track). It is passed through to the source untouched.
- Host API (eseqlisp): `register_dyn_word_source(name, Box<dyn Fn(&Value
  context) -> DynWords>)`, where `DynWords` is a list of groups
  `{group, items: [{word, detail}]}` plus an epoch. The widget asks only when a
  popup or field opens, or when validating after an edit or a `:dyn-context`
  change, and caches the answer per (source, context, epoch).
- Completion on a `dyn` slot is fuzzy (subsequence) over `word` and `detail`,
  grouped like the tracker's "+" picker. So typing `cut` finds
  `instrument:cutoff`, and `filt fr` finds `fx2:filterbank:freq`.

### 5.2 The `param` source (sequencer)

Registered by the sequencer UI. Context = destination track index (or `nil`).
Items are the targets the tracker's "+" picker lists (its legacy
`SEQ.track-lock-targets`, now the track's devices' params and rack macros
read through the kinds): Step, instrument, FX, MIDI FX, and rack macro
groups for that track, with each item's `word` set to its
`process_param_target_label` and `detail` set to its display label and
range.

*Built (eseq-jplk.5, `crates/sequencer/src/ui/param_words.rs`):* words are
read from the latest scheduler snapshot (the descriptors the engine resolves
against), cached per track on the UI thread, and the epoch is bumped by a
per-tick check that re-fingerprints cached tracks after each snapshot
publish. Groups: instrument, `FX N · name`, `MIDI FX N · name`, `Macros`,
`Step`, and on a rack track one `Slot N · instrument` group per slot (its
instrument's params as `rackN:instrument:…`, then `rackN:gain` / `pan` /
`base-note` / `max-polyphony` / `mute` / `solo`); detail = display name + the
range values are clamped to. Slot-free spellings are `DynWords.aliases`
(valid, never listed). Send labels are not offered (the engine skips them).
Rack-slot values ride the trigger as `StepEvent::rack_slot_params`, resolved
by name at landing against the slot's instrument descriptor (the UI engine
registry mirrored into `SequencerSnapshot::engine_instrument_descriptors`),
and beat the slot's stored p-lock and macro mappings for that hit only. The kind row shows *"not on
<track>"* beside the row while an invalid `plock` item is hovered.

### 5.3 Jaki kind wiring

- `row-schema` (`alez.jaki.doc`) gains
  `(list "form" "plock" (list "fixed" (list "dyn" "param")) value-schema)`,
  and `on-body` gets the same form.
- The row's `sexp-slot` passes `:dyn-context` = the row's destination track:
  `:route` mapped through `jk-route-tracks` (rack member → member track). A row
  with no route passes `nil`.

## 6. Reroutes and mismatches

A name is **valid** if the context's source lists it (slot-free spellings
match by effect/fx name). When the row's route changes, `:dyn-context`
changes and the slot revalidates:

- **Editor:** an invalid name is drawn with the slot's `error-color` band, and
  hovering it says *"not on <track name>"*. The text is kept, so routing back
  makes it live again. No automatic rewrite.
- **Runtime:** skipped per hit (§3). The rest of the row plays.
- `nil` context (unrouted row): names are not validated, and completion shows
  nothing but a hint row: *"route this row to see its parameters"*.

## 7. Non-goals (v1)

- Holding a value between hits (latch/glide). `plock` is per-hit, like a
  p-lock. A held automation lane is a different feature.
- ~~Range-aware value rails~~: built in `.6`, see §7.1.
- `send:*` per hit. The bus-send path has its own live-value restore
  semantics; it parses but is skipped at landing until a follow-up says how a
  per-hit send interacts with the fader.
- Hand-written `def-sequencer`s get `seq-emit :params` for free, but no
  editor support.

### 7.1 Range-aware value rails (built, eseq-jplk.6)

The value of `(plock NAME V)` scrubs, snaps, clamps and formats in NAME's
range. `alez.jaki.doc` spells the plock value's number leaf as
`(dyn-num "param" (num :min -100000 :max 100000 :step 0.01 :decimals 2))`
— in the bare value, in every `seq` level and in cycle lists — for both
`row-schema` and `on-body` (docs/sexp-slot-spec.md §4.1). The `param` source
(`ui/param_words.rs`) gives each item and alias a `DynItem::num` from its
descriptor (`param_num_spec`):

| param | rails |
|---|---|
| enum, boolean | descriptor min..max, step 1, 0 decimals |
| continuous, exponential (min > 0) | min..max, step a tenth of the bottom decade (20..20000 Hz → 1, 0 decimals) |
| continuous, linear | min..max, step ≈ span/100 as a power of ten, at most 1 (0..1 → 0.01, 2 decimals) |
| step param | `StepParam` min..max; step 1 where the lane nudges by whole units, else 0.01 |
| rack macro | 0..1, 0.01 |

The range is the descriptor's own min..max: the same stored units the engine
clamps to at landing and the detail text shows, so the rails agree with
playback. An unknown name, a `nil` context, or a name the track lacks falls
back to the wide rail. Stored values outside the rails are shown as stored
and kept (a reroute never rewrites data); only what the user scrubs or types
is clamped.

**eseq-jplk.7 (open).** A percent knob (`unit "%"`) stores 0..1 while the
knob and tracker show 0..100. Today both the engine and these rails use the
stored 0..1. If `.7` makes plock values knob units (0..100) and converts at
landing, `param_num_spec` must follow: a `%` param's rails become 0..100,
step 1 (or 0.1), 0 (or 1) decimals, and the detail text likewise — one
place, since the item carries its rails.

## 8. Open questions

1. **Units.** §3 picks raw units (knob and tracker units). The alternative is
   normalized 0..1, which is portable across synths but unreadable next to
   the knob. Revisit after first use.
2. **Rack routes.** A row routes to one member, so the context is one track.
   If rows ever route to "all pads", completion would need the union or the
   intersection of their parameters.

## 9. Test plan

- `ParamRef` round-trip against `process_param_target_label` for every
  variant, plus the slot-free spellings and rejection of `process:…`.
- Scheduler: a generator emitting `:params` for an instrument param and an
  FX param lands both on the event, beats a stored p-lock on the same step,
  and does not leak to the next hit. An unknown label on the destination
  plays the note and drops only that param.
- Rack: a rack-owned jaki `plock` resolves against the member pad's
  instrument.
- Jaki: `(plock L (seq :hit a b c))` walks per hit across cycles. Two
  `plock`s with different labels keep independent counters. `(on left
  (plock …))` locks only left hits.
- Editor (eseqlisp): `(dyn src)` completions come from a stub source, are
  cached per context, and revalidate on a `:dyn-context` change. An invalid
  word renders as an error. A capture fixture shows the popup on a
  jaki row.

## 10. Beads

| bead        | slice |
|-------------|-------|
| `eseq-jplk.1` | `ParamRef` parser + round-trip test (§2.1) |
| `eseq-jplk.2` | `seq-emit :params`, landing-time resolution and upsert, skip unmatched (§3). Needs .1 |
| `eseq-jplk.3` | jaki `plock` word: defer, `:params`, emit, `on` support, manual entry (§4). Needs .2 |
| `eseq-jplk.4` | eseqlisp `(dyn SOURCE)` schema atom, `:dyn-context`, source registry, fuzzy completion, invalid marking (§5.1). Independent |
| `eseq-jplk.5` | sequencer `param` source + jaki row schema and context wiring + reroute revalidation (§5.2, §5.3, §6). Needs .3, .4 |
| `eseq-jplk.6` | range-aware value rails for the `plock` value (§7). Needs .5 |
