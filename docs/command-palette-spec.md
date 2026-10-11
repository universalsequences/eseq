# Command Palette + Definition Docs — spec rev 1

Status: rev 1, 2026-10-10. Phase 1 BUILT (uncommitted) 2026-10-10. Epic: see `bd show` for the `command-palette` epic.

## 1. Goal

A modern, native-feeling command palette (search field, ranked rows, right
column showing current value / key binding, `›` sub-pickers with a badge in the
search field) that **is** M-x, not a parallel system. Anything a user defines in
`init.lisp` with the right header shows up automatically, which keeps the Emacs
property that the editor is extended in its own language.

Reference shape (from a terminal multiplexer's palette):

```
┌──────────────────────────────────────────────┐
│ 🔍 Search commands…                          │
│ ▌Change Theme                  Merino Dark › │   ← :current, has :choices
│  Rename Tab                                ›  │
│  New Tab                                  ⌘T  │   ← key binding (reverse lookup)
└──────────────────────────────────────────────┘
          ↓ Enter on "Change Theme"
┌──────────────────────────────────────────────┐
│ [Theme] Select Theme…                        │   ← badge = stage prompt
│ ▌Merino Dark                             ✓   │   ← ✓ = :current
│  Abyssal Trench                              │
└──────────────────────────────────────────────┘
```

## 2. Today

- Rust natives carry `SymbolMetadata { signature, docs, keyword_args }`
  (`crates/eseqlisp/src/runtime.rs`). Lisp `def` forms carry nothing.
- `collect_mx_candidates` (`editor/minibuffer.rs`) offers builtins + every VM
  global + every macro + bound handler names, unfiltered. Choosing a function
  that needs arguments fails at call time.
- `MinibufferMode::CommandChoices { title, entries, … }` is a one-level picker
  over `HostCommand`s — the seed of the sub-picker, but minibuffer-rendered and
  host-command-only.
- Theme switching is N zero-arg `seq-theme-*` functions plus a registry list in
  `content/ui/themes.lisp`.

## 3. Definition header (phase 1)

A function `def` may begin its body with a **header**: alternating keyword /
value pairs drawn from a closed set of known keys.

```lisp
(def seq-theme-pick (theme)
  :doc "Switch the UI theme. Moving through the list previews it live."
  :command "Change Theme"
  :choices seq-theme-choices
  :current (fn () seq-current-theme)
  (seq-apply-theme theme))
```

Rules:

1. Header keys are recognized only at the **head of the body**, only from the
   known set (§3.1), and only while at least one body form remains after them.
   `(def f () :doc "x")` therefore stays a function returning `"x"` — same
   rule Emacs applies to a lone docstring.
2. An unknown keyword at the head of the body ends the header (it is body, as
   today). A non-string `:doc`/`:command`/`:prompt`/`:group`, or a repeated
   key, is a compile error.
3. Header values are **not** evaluated at definition time except as noted:
   `:doc`, `:command`, `:prompt`, `:group` must be string literals;
   `:choices`, `:current` are expressions captured as zero-arg thunks (a bare
   symbol is the common case and means "call this function").
4. The header compiles to nothing in the function body. It is recorded as
   metadata keyed by the definition's **qualified global name** and harvested
   by the VM alongside warnings/macros after each compile unit.
5. Redefinition replaces the metadata (hot reload). Removing the header on
   reload clears it.
6. The signature is derived from the arglist (`(seq-theme-pick theme)`), so
   docs need not repeat it.
7. Multi-line docs: strings may span lines and support `\n` `\t` `\"` `\\`
   escapes (the lexer gained escapes alongside this spec; unknown escapes
   keep their backslash). The first non-empty line is the summary.

Hot-reload, the module-export migration, and the patcher projection all match
`(def name (args) …)` by shape and ignore the body, so the header needs no
changes there.

### 3.1 Known keys

| key        | value              | meaning |
|------------|--------------------|---------|
| `:doc`     | string             | Docs. First line = summary shown in palette rows and completion. |
| `:command` | string             | Marks the function as a palette command; the string is its human title. |
| `:prompt`  | string             | Badge noun for the choice stage (default: the parameter name, title-cased). |
| `:choices` | expr → list        | Entries for the argument. Presence makes the row show `›`. |
| `:current` | expr → value       | Current value: right column on the row, `✓` in the sub-picker. |
| `:group`   | string             | Optional section ("View", "Tracks", …) for the empty-query list. |

`:doc` is the only key phase 1 needs. The others are parsed in phase 1 so the
grammar ships once, and gain behavior in phases 2–4.

### 3.2 Choice entries

`:choices` returns a list whose items are either plain values (label = `str`
of the value) or dicts:

```lisp
(dict :label "Merino Dark" :value "merino-dark" :detail "dark · warm")
```

The selected entry's `:value` (or the plain item) is passed as the command's
**first argument**. This is what turns
`(def add-track-with-instrument (instrument) …)` into
**Add Instrument › [type to filter instruments]**.

### 3.3 Docs surfaces

- `completion_metadata` / `completion_symbols` merge Lisp header docs with
  native docs, so editor completion shows them for free.
- `describe-function` (new M-x command): opens a read-only `*Help*` buffer
  with signature, docs, command title and choice prompt. Source location and
  key bindings land with phase 3's reverse binding lookup.

## 4. What counts as a command (phase 2)

`:command` is Emacs's `(interactive)`: the curation marker. A function is
*callable from the palette* when it has `:command` and either takes no
required arguments or declares `:choices` for its single required argument.
Violations are compile warnings (not errors — user init must keep loading).

M-x stays the escape hatch: it still offers every global, but ranks commands
first and filters out globals whose required arity can't be satisfied.

## 5. Palette ranking (phase 3)

One list, three tiers:

1. **Empty query:** pinned seed commands, then recent/frequent commands
   (usage counts persisted in user data, decayed). Optionally grouped by
   `:group`.
2. **Typing:** fuzzy match over `:command` titles, then over their names;
   row subtitle = first line of `:doc`; right column = `:current` value or the
   key binding (reverse lookup of `lisp_bindings` + builtin binds).
3. **No command match / prefix `(`:** every documented global by raw name,
   then undocumented globals — today's M-x behavior.

## 6. Choice stages (phase 4)

- Enter on a row with `:choices` pushes a stage: the search field shows a
  badge (`:prompt` or the title-cased parameter name) and the placeholder
  `Select <Prompt>…`.
- Typing filters entries; `✓` marks the `:current` value; Enter calls the
  command with the entry value; Backspace on empty input pops the stage; Esc
  closes.
- Optional live preview: a command may declare `:preview fn` (phase 4.1),
  called on selection change and reverted on Esc. Themes are the first user.
- Multi-argument commands (later): `:choices` per parameter
  (`:choices (instrument instrument-choices preset preset-choices)`); badges
  stack as breadcrumbs **Instrument › Preset**. Not in rev 1 scope.

## 7. UI (phase 3)

Rendered in Lisp as a window-level overlay, opened by the palette hotkey and
by M-x (M-x can keep the minibuffer skin behind a customize toggle).

Risk: today a modal receives pointer input only through the **active** tile
(see `content/ui/choose-model.lisp`). A global palette needs a window-level
overlay that takes keyboard focus regardless of tile; settle that seam before
the visual work.

## 8. Backfill

Do not document the ~12k defs. Document what people actually run: theme
commands, track/rack add commands, customize, file/export/import,
choose-model, packages. A test pins that every `:command` has `:doc`.

## 9. Phases

1. Header parser + metadata harvest + completion/docs surfaces + `describe-function`.
2. `:command` semantics, arity check, M-x ranking/filtering.
3. Palette overlay UI + ranking + usage counts + key binding column.
4. `:choices` / `:current` / `:prompt` stages; convert themes to one
   "Change Theme"; add "Add Instrument".
5. Backfill docs/commands for the frequently-used set.
