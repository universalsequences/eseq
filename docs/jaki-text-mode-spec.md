# Jaki Text Mode — schema-driven completion for live-coding `jak` forms

Status: rev 1, 2026-10-01 — design only, nothing built. Epic and children
listed in §9.

## 1. Idea

The jaki kind panel is easy to play because every position offers exactly
what fits there: the figure dropdown, the route dropdown, a row's sexp-slot
popup of words and forms, `(plock …)` names of the routed track's
parameters, numbers that scrub within their range. None of that knowledge
lives in widget code. It is data: `alez.jaki.doc` exports `row-schema`,
`proc-trigger-schema`, `proc-body-schema` and `figure-option-labels`, and
the host's `(dyn "param")` / `(dyn "track")` word sources supply the live
names.

`jaki-text-mode` gives a plain text buffer of `(jak …)` forms the same
affordances. You type the form yourself, and the completion popup offers what
the panel's dropdowns would at that position:

```lisp
(jak "kit" :16
  . . - . (every 2 swap)
  -> 0 (every 2 |          ; popup: rev swap stac ghost … fast note …
  -> 1 (plock "cut|        ; popup: instrument:cutoff  (track 1's params)
  -> 2 (on |               ; popup: left right dot dash … fig rep not and or
```

## 2. Constraint: the mode is a package

Jaki is a package, not core. **All jaki knowledge lives in Lisp in
`content/packages/alez.jaki/`.** No Rust may know about `jak`, `->`,
figures, row schemas or jaki routes.

That constraint is met, but it does not mean zero Rust. Today a package
cannot drive the completion popup at all:

- The popup is computed in Rust (`crates/eseqlisp/src/mode.rs:238`,
  `completion_match_with_extras`) from the symbol prefix at the cursor.
  A mode's own items (`mode-add-completions`) are a static, append-only
  list filtered by prefix. Nothing lets a mode say "at this position, these
  items".
- The popup opens only on a symbol prefix (`has_completion_prefix`), never
  after `(` or a space, which is where the panel's "everything relevant"
  list matters most.
- Lisp can read `current-line-text` and `current-line-number` but not the
  cursor column, and the host exposes `dyn-word-valid?` but no way to list a
  source's words.

So §4 adds **three small generic seams** to the editor and host. Any package
mode can use them (expr-buffer, a future tracker text mode…). They carry no
jaki semantics, and everything in §5–§8 is package Lisp.

## 3. What the panel offers, and where it comes from

| position in a `jak` form          | panel affordance                | source (all Lisp data today)            |
|-----------------------------------|---------------------------------|-----------------------------------------|
| pattern, before the first `->`    | figure dropdown                 | `alez.jaki.doc/figure-option-labels`    |
| right after `->`                  | route dropdown (track names)    | `(dyn "track")` host source             |
| a route's words                   | row sexp-slot popup             | `alez.jaki.doc/row-schema`              |
| `(plock NAME …)` NAME             | fuzzy list of the track's params| `(dyn "param")`, context = route track  |
| a number argument                 | scrub within rails              | schema `num` / `dyn-num` rails          |
| `(rule TRIG …)`                   | rule trigger / body slots       | `proc-trigger-schema`, `proc-body-schema` |
| an invalid word                   | error colour                    | schema check / `dyn-word-valid?`        |
| the sounding hit's items          | `:lit` / `:lit-values` rings    | the generator's `<route>` marks (§7.3 of the sequencer spec) |

In text, a route is `-> N` and N is a track number directly
(`alez.jaki.surface` header). So the `param` source's context for a route is
simply N. The kind panel needs `jk-row-track` to map a rack pad to a member
track. A text route does not.

## 4. Generic seams (Rust, not jaki-specific)

### 4.1 `define-mode … :complete fn-name`

The editor calls the mode's completer whenever the buffer text or cursor
changes while the buffer is in that mode (or a mode that inherits it):

```lisp
(fn-name before)   ; before = text from the start of the enclosing
                   ; top-level form up to the cursor ("" at top level)
```

It returns `nil` to fall through to ordinary symbol completion, or:

```lisp
(dict :replace 3          ; chars before the cursor a pick replaces
      :items (list (dict :label "swap" :detail "" :group "words" :doc "…")
                   (dict :label "" :detail "route this row to see its parameters")) ; hint
      :filter :none)      ; the mode already filtered; editor shows as-is
```

- Items with an empty `:label` are hint rows: shown dim and not selectable.
  This mirrors `DynItem` hints in `crates/eseqlisp/src/sexp_slot/dyn_words.rs`.
- `:group` renders where `category` does today, and `:detail` beside the
  label. Both use the existing `CompletionItem` fields, so the renderers
  (`ui/backend.rs:601` `CompletionEntry`, TUI, Metal) need no new layout.
- The popup opens whenever the completer returns non-empty items, including
  right after `(` or a space. A non-empty list is how the mode opts in to
  auto-open. Esc closes the popup until the next edit, as it does today.
- Cost: one Lisp call per edit, on the UI VM. The completer must stay cheap
  (§5.3). The editor skips the call while a key repeat is coalescing, as it
  does for the existing refresh.

`before` is computed in Rust with the existing top-level form scanner
(`mode.rs` `enclosing_form_tokens_before_cursor` / `top_level_form_tokens`),
so a package never reads the whole buffer per keystroke.

### 4.2 `(cursor-column)`

Returns the 0-based byte column of the cursor on its line. It is the missing
partner of `current-line-number` and `current-line-text`, and number nudging
(§7) needs it.

### 4.3 `(dyn-words source context)`

Lives in the sequencer host (`crates/sequencer/src/ui/param_words.rs`) next
to `dyn-word-valid?`. It returns the source's answer as plain data:

```lisp
(list (dict :group "Instrument"
            :items (list (dict :word "instrument:cutoff" :detail "Cutoff"
                               :min 20 :max 20000 :step 1 :decimals 0) …))
      …)
```

It goes through the same per-(source, context, epoch) cache the sexp-slot
uses. A `nil` context returns the hint group. Number rails appear only on
items that have them (`DynItem::num`).

### 4.4 Scrubbable numbers: drag the text itself

Numbers in the buffer become draggable **as the text they are**. No widget
is drawn, nothing is inserted, and there is no `~` form. The glyphs on screen
are the value, and dragging them rewrites them in place, like scrubbing a
number in Bret Victor's demos or Tributary. **Syntax highlighting is the
affordance**: a number you can drag is coloured differently from one you
can't, all the time, so you can read what's playable at a glance, not only
on hover. This is deliberately not the inline-widget layer
(`docs/inline-code-widgets-spec.md`): that one draws controls beside the
code, and this one only changes how existing text looks and reacts.

**The package publishes the spans; the editor owns everything else.**
Deciding *which* numbers are scrubbable needs jaki knowledge, so it is Lisp.
It can't be asked per number per frame (highlighting runs every frame over
visible lines), so the package computes a span table after edits and hands
it over once:

```lisp
(set-buffer-scrub-spans
  (list (list line start-col end-col min max step decimals) …))
```

- Spans are 0-based line plus byte columns `[start, end)` over a number
  token, with its rails.
- Replacing the table is the only operation: an empty list clears it.
- **Highlighting:** a number token covered by a span draws in a new theme
  slot `:syntax-scrub-number` instead of the ordinary number colour
  (`TokenClass::Number`). Nothing else changes: same font, no underline, no
  box. The slot goes into every complete theme (`content/ui/themes/`), like
  any new slot.
- **Staleness:** between an edit and the package's next table, the editor
  shifts spans through the same edit-adjustment it uses for cursors and marks.
  A span whose text is no longer a number is dropped, so a half-typed number
  goes back to plain number colour until the next pass re-marks it.
- **When the package recomputes:** the mode's `:on-change fn-name` hook
  (debounced, ~100 ms idle after an edit, plus once on mode entry) reruns
  the segmenter over the visible `jak` forms. Validation (§7) uses the same
  pass and hook.

The gesture uses only the table, so no Lisp runs per pixel:

1. **Hover** over a span shows the vertical-resize cursor.
2. **Press** on a span and **release without moving** (under ~3 px) places
   the cursor as usual, so text editing is unaffected. **Moving past the
   threshold** starts a scrub. Vertical motion: up increases. Shift gives
   fine steps (÷10 of `:step`, one more decimal shown). Sensitivity: the
   whole range spans about 200 px, but never less than 4 px per `:step`, so
   small integer ranges feel stepped rather than twitchy.
3. **Each step change rewrites the token text** in place (formatted with
   `:decimals`, snapped and clamped like `NumSpec::snap`). The span tracks
   its own new width, and the rest of the line reflows naturally. The whole
   gesture is **one undo group**, and the cursor and selection are adjusted
   like any edit. The number being dragged gets the selection background
   while the gesture lasts.
4. **Release** calls the mode's optional `:on-scrub-end fn-name` with no
   arguments, and the package decides what to evaluate.

Live sound during the drag: the editor also calls `:on-scrub fn-name` (if
present) after each value change, at most every 100 ms. Jaki uses it to
re-evaluate the enclosing `jak` form. Whether that is cheap enough per change
must be **measured before relying on it**: buffer re-eval re-expands the
macro and republishes the sequencer (`eseq-wlc` timing wobble). If it isn't,
v1 evaluates only on release and live preview becomes its own bead.

## 5. `alez.jaki.text` — the package module

A new module `content/packages/alez.jaki/src/text.lisp`. It imports
`alez.jaki.doc` (for the schemas) and holds no widget code.

### 5.1 Partial reader

`(read-partial before)` tokenizes the text before the cursor into a stack of
open lists. Each frame records its head and the complete items so far. The
result also carries the trailing partial token and whether the cursor is
inside a string. It never fails on unbalanced input, because that is the
normal case while typing. Lexer facts follow the jaki spec §4.1: `.` and `-`
are bare symbols, `->` is one symbol, and `"…"` is a string (plock names).

### 5.2 Schema cursor walker

`(schema-context schema frames)` walks a schema (plain Lisp data, the
spellings in `crates/eseqlisp/src/sexp_slot/schema.rs` §4: `num`, `word`,
`or`, `form`, `forms`, `fixed`, `rest`, `dyn`, `dyn-num`) down the reader's
open frames. It returns what the cursor position expects:

- `:head` — just after `(`: the form names valid here, then words.
- `:value` — an argument position: the slot's words, its `dyn` sources and
  its num rails.
- `nil` — no schema position (e.g. a number typed where only forms fit).

This duplicates the semantics of Rust's `Schema::completion_context`
(`schema.rs:499`) on purpose, because the package must not depend on new
core natives for jaki behavior. To keep the two from drifting, a lisp_host
test feeds a shared table of `(schema, text, expected)` cases to both (§8).
The Rust walker is the reference. If they disagree, the Lisp side is the bug
unless the sexp-slot spec changes.

### 5.3 Segmenter

`(jak-position frames)` places the cursor in the `jak` grammar
(`alez.jaki.surface` header):

1. Not inside a `(jak …)` form → `nil` (normal completion).
2. Inside a voice line or the jak body, **before the first `->`** → pattern
   position: figure words from `figure-option-labels` (`.` `-` and the
   figure transforms), with `pat` grammar forms such as `(every n …)`.
3. **The token right after `->`** → route position: track numbers from
   `(dyn-words "track" nil)`, with names as `:detail`.
4. **After `-> N`** → row position: the frames from the last `->` onward
   become one `row-schema` value (`forms` row). Context for `dyn` lookups is
   N. Inside `(rule …` the trigger slot uses `proc-trigger-schema` and the
   rest uses `proc-body-schema`.

Cheapness: the reader only sees the enclosing top-level form (§4.1), and
schemas are module-level values, so a call costs O(form length + schema
depth). Measure on a 40-line `jak` with the completion trace
(`trace_completion_enabled`). The budget is under 1 ms on the UI VM.

### 5.4 The completer

```lisp
(define-mode "alez.jaki.text/jaki-text-mode" :inherit "eseqlisp-mode"
  :complete "complete")
```

`complete` runs reader → segmenter → walker. It then builds items:

- Static words are filtered by prefix, like the slot.
- `dyn` words are filtered by fuzzy subsequence over word and detail, like
  `completion_rows` (`edit.rs:450`).
- It returns `:filter :none`. `:replace` is the partial token's length; for
  a plock name, the partial string minus its opening quote.

Inheriting `eseqlisp-mode` keeps eval, keys and highlighting. Outside a
`jak` form the completer returns `nil`, so ordinary completion still works
in the same buffer.

Picking a form name inserts `(name ` plus the schema defaults the slot
pre-fills (`(every 2 rev)`, `(acc :by 1 :min 0 :max 8)`). Picking a word
inserts the word.

## 6. Entering the mode

- `(import alez.jaki.text)` registers the mode. `M-x jaki-text-mode` or a
  package menu entry switches the current buffer to it.
- Files ending in `.jak.lisp` open in it (if the editor has an
  extension→mode table; otherwise only the command).
- A "Open as text" item on the jaki kind panel opens a scratch buffer
  holding `jk-code-lines` (the pattern as the `jak` form it plays) in
  jaki-text-mode. That gives a one-step path from GUI to text. Writing text
  back to the kind document is a non-goal (§10).

## 7. Beyond the popup (later phases)

- **Number scrub** (§4.4). On `:on-change`, the segmenter walks every
  visible `jak` form, and each number at a schema `num` position becomes a
  span with that slot's rails: `(every 2 …)` gets 1..16 integer steps, `(vel 0.7)` 0..1 by 0.05,
  `(note+ 3)` its semitone range. A plock value uses the `dyn-num` rails of
  its parameter from `dyn-words` (cutoff scrubs 20..20000 in Hz). A number
  outside any schema position (pattern-grammar numbers before the first `->`
  are not schema'd) gets no span and keeps the plain number colour. `:on-scrub` /
  `:on-scrub-end` re-evaluate the enclosing `jak` form. The same rails also
  drive `M-up` / `M-down` at the cursor (`cursor-column`), as a keyboard
  fallback.
- **Validation.** On idle after an edit, check each `jak` route segment
  against `row-schema` with the walker's check pass, and plock names with
  `dyn-word-valid?`. Mark failures with `set-buffer-styles` in the error
  colour. Text is never rewritten.
- **Lit items** (stretch). The running generator already marks
  `<route>` with bitmasks of which route items applied to
  the sounding hit. Mapping item i of route r to its source span (kept
  by the segmenter during the validation pass) and styling it with
  `set-buffer-styles` gives the panel's rings in text. This needs the mark
  to reach the UI VM per frame; check its cost before committing.

## 8. Tests

- eseqlisp: `:complete` hook unit tests in `editor/tests.rs` (nil falls
  through, hint rows unselectable, auto-open after `(`, `:replace` span).
- sequencer lisp_host: `dyn-words` returns groups/rails for a routed track
  and the hint for `nil`.
- lisp_host: walker parity table, with the same cases run through
  `Schema::completion_context` and `alez.jaki.text/schema-context` (§5.2).
- lisp_host: segmenter table covering pattern / route / row / rule / plock
  positions, single- and multi-voice `jak`.
- One editor-level test: a buffer in jaki-text-mode, type
  `(jak "k" :16 . -> 0 (every 2 ` and assert the popup's labels.

## 9. Beads

Epic `eseq-jtxt` — see `bd show` for the children and their dependencies:

1. `:complete` mode hook (eseqlisp, generic) — §4.1
2. `cursor-column` + `dyn-words` natives (generic) — §4.2, §4.3
3. `alez.jaki.text` partial reader + schema walker + parity tests — §5.1, §5.2
4. jaki-text-mode segmenter + completer (static words) — §5.3, §5.4, §6
5. plock / route `dyn` completion — §5.3 steps 3–4 with `dyn-words`
6. scrubbable numbers: generic span table + highlight slot + drag gesture (§4.4), jaki span pass (§7)
7. validation styling — §7
8. lit items in text (stretch) — §7

## 10. Non-goals

- Round-tripping text edits back into a jaki kind document. The kind panel's
  document is the panel's; text mode authors `jak` forms.
- A structural (paredit-style) editor. The sexp-slot already is one.
- Rust-side knowledge of jaki. If a later phase seems to need it, the answer
  is another generic seam, or the phase is wrong.
