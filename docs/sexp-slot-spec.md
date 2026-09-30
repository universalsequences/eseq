# sexp slots: Lisp on guard rails

Status: design rev 2, 2026-09-28, from an interview. Not built.
First consumer: the jaki kind (docs/jaki-kind-spec.md). Intended for any panel.

## 1. What it is

A **slot** edits one value that is a tiny, guarded piece of Lisp: a number, a
word, a list of those (nesting allowed), or a **form**
(a head word plus guarded args, like `(every 2 (rev swap))`). You *type* to
create and reshape; you *scrub and pick* to tweak. The screen always shows the
structure: numbers are number-pickers, words are dropdowns, lists are real
paren glyphs around their elements.

```
typed:   (1 2 3 4)⏎
shown:   ( [1] [2] [3] [4] )          four number-pickers in parens

row:     [2 Snare ▾]  (trunc (3 1))  right  (every 2 (rev swap))  [+ …]
                       ^head ▾ ^list          ^head  ^num ^list of words
```

It edits data and knows nothing about jaki. A host gives it a **schema** (the
guard rails) and decides what a list *means*.

It is **one native widget** (`sexp-slot`, Rust, next to number-picker and
dropdown). The whole slot takes focus as a unit and manages its own internal
cursor, keys, typing, parsing, validation and popups. Lisp never sees or
manages the internals: it passes a schema and a value and receives one
`on-change` per committed edit.

## 2. Values

| Atom | Typed as | Shown as | Stored as |
|---|---|---|---|
| number | `3`, `-1.5` | number-picker (min/max/step/decimals from schema) | number |
| word | `rev`, `:16t` | dropdown of the schema's word set | string (a `:`-prefixed word is emitted as a keyword) |

Timebases are not a separate kind: `:16 :16t :8n …` are simply words in a
schema's word set. Ratios are out for now (keep it simple).

A **list** is `(` elements `)`, elements are any value the schema allows,
nesting is free, and a list is never empty. A **form** is a list whose first
element is a head word from the schema, followed by that head's args.

Transposes stay plain numbers: `(note (1 4 6 9))` is "transpose 1, then 4,
then 6, then 9". No scales or note names yet (possible later atoms).

## 3. Lists mean "one per tick of the host's clock"

The slot only edits data. The **host** decides what advances a list and the
schema names it so the UI can say it:

| Host | List clock |
|---|---|
| jaki (route args, words) | per cycle (jaki's implicit `cyc`) |
| neural node param, e.g. delay `(1 4)` | per fire of that node (future, needs engine support) |
| step / p-lock values | per pass of the step (future) |

## 4. Schema (the guard rails)

A data s-expression the widget reads. It picks the atom widget, validates
typed input, and feeds completions and `+` menus.

```lisp
(num :min 1 :max 16 :step 1 :decimals 0 :default 4)
(word rev swap stac ghost)                ; first word is the default
(word :16 :16t :8 :8t :4)                  ; timebases are just words
(or (num :min 0 :max 1) (word off))        ; either atom
(form every (num :min 1 :max 16) (word rev swap stac))
(forms                                     ; a row: any of these, in order
  (word left right accent rev stac ghost swap)
  (form trunc (num :min 1 :max 32 :default 3))
  (form every (num :min 1 :max 16) (word rev swap stac ghost))
  …)
(fixed …)                                  ; opt out of list-ness for one slot
```

- Every atom slot also accepts a **list of itself** (and lists of lists)
  unless it is `(fixed …)`. That is what makes `(fast (1 2 3))` and
  `(every 2 (rev swap))` fall out with no per-head work.
- `:clock` on the outer schema (`per-cycle`, `per-fire`, …) is only a label.
- `(rest <schema>)` is only a form's last arg: the form then takes one or
  more trailing values of `<schema>`. `(form seq (word :hit :cycle) (rest
  (num …)))` accepts `(seq :hit 0 3 7)`; `+` inside its `)` adds a copy of
  the last value, Backspace removes trailing values down to one.
- A form is recognized by the schema that applies **where it sits**, not the
  root's: a `(seq …)` in a `(note …)` arg or a `(fig 2)` in an `(on …)`
  selector is a form of that position's schema (head drawn as a head, no
  `+` unless its last arg is `rest`).

## 5. Interaction

Balanced: typing creates and reshapes, the mouse tweaks.

**Typing into a slot**
- A focused number-picker already takes digits (type, Enter commits, clamped).
- Typing `(` on a focused atom turns that atom into a text field seeded with
  `(`. Type `(1 2 3 4)` Enter: it becomes four number-pickers in parens.
- Enter on a focused list's `(` opens the whole list as text (`(1 2 (3 4))`);
  Enter re-parses, Esc restores.
- While typing, a small completion list under the field shows what the schema
  allows at the cursor (heads, words), filtered as you type; Tab or
  Enter accepts.
- Missing trailing `)` close at the end, so `(1 2 3 4` Enter is a list.
- Rules on Enter: text must parse as exactly one s-expression. Numbers out of
  range **clamp** (40 into max 16 becomes 16). A wrong type or an unknown word
  **rejects**: the field stays open with a short red reason. Nothing is
  silently dropped.

**Structure without buttons** (no × anywhere: it looks bad)
- Backspace on a focused element deletes it. (The picker uses Backspace only
  while typing a number, so this does not collide.)
- A list left with one element collapses to that plain value.
- A small `+` just inside `)` appends a copy of the last element; scrub or
  retype it.
- A form's head is a dropdown of the heads with the same arg shape
  (trunc / rot / shift all take one number), so args survive a head swap.

**Adding a form to a row**
- The row's `+` becomes a text field when focused or clicked, with schema
  completions under it: type `right` or `(every 2 (rev swap))` and Enter
  appends the form. Picking a completion inserts the head with default args.
  This replaces today's separate filterable `+` menu (one entry point).

**Focus and the internal cursor**

The slot is one focus stop. Inside it a **cursor** sits on one item: an atom,
a list's `(`, a form's head, or a `+`. The cursor is the widget's own state
(kept per widget id, like the number-picker's typed-edit state), never Lisp's.

| Key | On the cursor item |
|---|---|
| Left / Right | previous / next item at the same level (siblings); at either end, focus leaves the slot through normal spatial focus |
| Up | out one nesting level: to the enclosing list's `(` (or form head) |
| Down | into the list or form at the cursor: its first element |
| Up / Down in an open choice popup | move the highlight (dropdown behaviour) |
| digits, `-`, `.` | number: typed entry, Enter commits (clamped) |
| a letter | opens the text field seeded with it, completions below |
| `(` | opens the text field seeded with `(` (list mode) |
| Enter | on a word: open its choices; on a `(`: the whole list as text; on a head: the whole form as text; on `+`: add; in a field or popup: commit |
| Backspace | delete the item (a one-element list collapses); in a field: delete a char |
| Tab | in a field: accept the highlighted completion |
| Esc | close the field / popup without change; again: leave the slot |

Values never change from arrow keys: numbers change by typing or dragging,
words by their popup.

**Mouse**: a click puts the cursor on an item and focuses the slot; dragging a
number scrubs it; clicking a word or head opens its choice popup
(the dropdown menu's look).

## 6. Look

- Real `(` `)` glyphs around the element widgets, dim, same font as labels.
- Each nesting depth sits on a slightly lighter band, so `(1 2 (3 4))` reads at
  a glance.
- Atoms keep their own widgets' look (picker, dropdown), compact (row height).
- A red reason under an open field; no other error chrome.
- **Width follows content.** A slot measures itself from its value, so a long
  list is never squeezed into a fixed box. `:width` is only a minimum. With a
  `:max-width` (or a parent that constrains width) it wraps between elements
  and grows taller rather than clipping; an open text field grows with its
  text the same way.

## 7. Host API (sketch)

```lisp
(sexp-slot
  :key "row-2-mods"
  :schema jk-row-schema          ; §4
  :value (get row :mods)          ; data (§2)
  :on-change (lambda (v) (jk-set-mods self 2 v)))
```

- The value is plain data, so it drops straight into a kind `:document` field
  (scene-locked, undoable, saved) or a `defscene`.
- View state (the cursor, the open field and its text, the completion
  highlight) is the widget's own, keyed by widget id; instance key scoping
  already makes those ids unique per instance.
- Every committed edit is one `on-change` call, so one undo step.
- `:on-hover (lambda (item) …)` (optional) hears the top-level item under the
  pointer, as its stored value, whenever it changes, and `nil` when the
  pointer leaves the item or the slot (the editor reports the leave through
  `widget_render::sexp_slot::pointer_moved_to`).
- `:wrap false` keeps the value on one line; the slot grows past its box
  instead of wrapping between pieces.
- `:tint-args '(("on" 0))` draws argument 0 of every `(on …)` form (its atoms
  and bands) in `:tint-color` (default the syntax string color), so a form's
  "where" reads apart from its "what".

## 8. Build plan

1. `read-string` native in eseqlisp: parse exactly one s-expression into data
   (numbers, symbols, keywords, strings, nested lists), never evaluate; nil
   plus a reason on failure. The widget uses the same parser in Rust.
2. Schema model in Rust (`crates/eseqlisp/src/sexp_slot/`): parse the schema
   value, validate / clamp / default a value, list completions at a cursor
   position. Pure, unit-tested.
3. `sexp-slot` widget (`widget_render/sexp_slot.rs`): layout of the value tree
   (paren glyphs, depth bands, atom cells), the internal cursor, key and mouse
   handling, the inline text field and completion / choice popups, emitting
   `on-change` with the new value.
4. jaki: row modifiers become one `(forms …)` slot per row; figure-level
   modifiers and figure cycling later.
5. Later hosts: neural node params (needs a per-fire list clock in the graph
   engine), step values.

## 9. Decisions closed in the interview

- A value that must never cycle is `(fixed <schema>)`: typing or building a
  list there is rejected with a reason. Nothing is fixed by default; hosts
  wrap the params that cannot cycle.
- Ratios are out; timebases are words; arrows never change values.
