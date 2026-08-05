# FX value-delta publishing (scene-switch re-eval elimination)

## Problem

Scene switch measured on kacrotest (2 rack tracks, fx panel open): one reactive
cycle of ~90ms, of which the `*fx*` buffer root effect re-eval is **85–87ms**.
Everything else (flush, relayout, other 8 buffers) is sub-millisecond; the
`switch-pattern` host handler is 7.5ms after the Arc/CoW round.

The re-eval fires because `SEQ.instrument-panel` / `SEQ.effects` are rebuilt on
the pattern/fx-epoch branches and the rebuilt trees bake per-scene device
values into leaf cells (`params[*].value`, `slots[*].gain`, `macros[*].value`,
`selected-instrument.params/synth[*].value`). `is_unchanged` fails on those
leaves, the buffer root effect (subscribed `scope=All` via its `each`) goes
dirty, and the whole fx tree — for racks: slot rows, pad grid, macro bank, the
*recursive* selected-slot instrument panel, chain fx — re-evaluates.

Trace evidence (ESEQ_SCENE_TRACE_DIFF, added in eseqlisp/src/reactive.rs): the
scene-to-scene diffs are **leaf value numbers only**; the widget structure is
identical. And every such value already reaches its widget through a per-param
reactive field binding that is independently re-synced on scene switch
(`track-{t}-fx-{s}-param-{i}-{name}`, `track-{t}-rack-slot-{s}-gain`,
`track-{t}-rack-slot-{s}-instrument-param-{i}-{name}`, `track-{t}-rack-macro-{id}`,
bus equivalents — all confirmed firing in the kacrotest trace). The 87ms
rebuilds an identical structure to carry numbers the binding layer already
delivers.

## Fix

New Runtime API in eseqlisp: `set_reactive_value_patch(namespace, field, value)`.

Key mechanical fact: `Namespace.fields` stores a `Value` whose inner
`Rc<RefCell<Value>>` cells are **shared** with the VM global namespace map and
with anything the Lisp side captured from previous evals (`Value::clone` is
shallow for Map/List). Mutating a leaf cell in place therefore updates every
view at once — including what `(get p :value)` logic reads on any *later*
eval — without any dirty marks.

Algorithm:

1. If field unregistered or values equal → behave like `set_reactive`
   (no-op fast path applies).
2. Walk stored vs candidate in parallel and classify diffs:
   - **Patchable**: same-variant `Number` or `Bool` leaf changed.
   - **Structural** (any one forces full fallback): list length change, map
     key added/removed, variant mismatch, `String`/`Symbol`/`Keyword`/other
     leaf changed, `Nil` transitions.
3. Any structural diff → delegate to plain `set_reactive` (full pipeline,
   dirty marks, re-eval). Honest structure changes (device add/remove, slot
   count, selected slot, track switch) take this path automatically, so the
   API is safe at any call site.
4. Only patchable diffs → `*cell.borrow_mut() = new_leaf` for each, **no**
   registry set, no dirty marks, no widget ids. Return a result flagging
   `patched: true` so callers/tests can assert the path taken.

Strings are structural on purpose: labels (`display-name`, macro `name`,
dropdown `text-value`) are baked into widget text, not binding-driven; a
silent in-place patch would leave stale pixels.

## Call sites (sequencer crate)

Switch publishing of `SEQ.effects`, `SEQ.midi-effects`, `SEQ.instrument-panel`,
`SEQ.bus-effects` to the patch API at the hot resync paths:

- `reactive_tick.rs` track/pattern-epoch branch (~line 317–336)
- `reactive_tick.rs` fx-epoch branch (~line 1243, 1303)
- `reactive_sync.rs` switch-pattern inline resync sites that set these fields

Authoring/load/event_loop sites keep plain `set_reactive` — they are genuine
structural moments and not hot.

## Caveats / consequences

- **Reader aliasing (spec rev 5 caveat)**: Lisp closures captured value cells
  at the last eval; in-place patch is exactly the aliasing we want (fresh
  reads), but any future code that assumes a published tree is immutable
  after `set_reactive` is wrong once this lands. The patch only ever writes
  whole leaf values into cells; it never restructures.
- **Dropdown params**: a scene change that flips an option-param value also
  changes its derived `text-value` string → structural fallback → full
  re-eval for that switch. Acceptable; follow-up would be binding-driven
  dropdown labels.
- **Rack macro name clobber**: kacrotest scenes restore stale macro *names*
  (`"Release" -> "Macro 1"` on every switch). That is a real bug AND its
  String diff would force the structural fallback every switch, masking this
  win. Fixed alongside (scene restore keeps live macro names; values only).
- The `*fx*` root effect keeps its `scope=All` subscription; nothing about
  subscription bookkeeping changes. A patched switch simply never dirties it.

## Verification

- eseqlisp unit tests: patch-path taken for value-only diffs (no dirty
  widgets/effects, subsequent `set_reactive` with identical value is
  unchanged); structural fallback for len/key/string/variant changes; lisp
  read-after-patch sees new values.
- kacrotest manual trace: `[ui-trace]` hot list must no longer show `*fx*`
  ~87ms on scene switch; expected switch cycle ≤ a few ms.
