# Customizing eseq

For requests about the app itself rather than about music: "make the mute
buttons red", "add a column to the step panel", "bind a key to …", "make the
mixer wider". Every panel is eseqlisp in a factory module (`eseq.*`). You change
one without editing its file: from outside, with an `override`, a knob, or a
theme delta.

## Where a change belongs

Decide this before writing anything. A small tweak is not a package.

| Home | Lasts | For |
|---|---|---|
| `try` (live, unsaved) | until Keep, Revert or restart | showing the user how a change looks |
| `user.init` (`~/.eseq.d/init.lisp`) | always, every project | personal preferences: a colour, a key, a knob value |
| a Local package | while attached | a named set the user turns on and off or shares (a whole alternative mixer look) |
| the project's scratch | that project only | what belongs to one set; the user writes it, not you |

The test: would the user give it a name and switch it on and off? Then make a
package. Otherwise it is how they like the app: `user.init`.

## The flow

1. Find what draws it (below).
2. `try` the change. It runs in the user's open app, as module `user.try`, so
   they see it at once. Later `try` calls replace a definition or override of
   the same name, so iterate freely.
3. Say what changed and where to look, and ask whether to keep it. The pane
   shows them **Keep** (the forms go into init.lisp) and **Revert**.
4. If they tell you in chat to keep it, call `keep_tried`: it does what Keep
   does, creating init.lisp if need be. To change something already in
   `user.init` later, `define_form` into `user.init`.

`eval` and `capture` run in the headless copy, not the user's app: an override
made there changes nothing they can see. Use them to check a layout before
`try`.

## Finding what draws it

- `search_source` for the words on screen, a widget `:key` from a `capture`
  layout, or a field name (`muted`), with `filter` set to the area (`mixer`).
- `form_source` the definition, then the helpers it calls. Override the
  smallest function that draws the thing, not the whole panel.
- `outline` a module for its functions. A module's `(export …)` lists its
  public ones; overriding a private one works but warns, since it may change
  on update.
- An installed package may already replace the panel (search finds its
  `override`s): ask which one the user means.

## override

```lisp
(import eseq.mixer)

;; Replace the function outright: the strip's buttons become one label.
(override eseq.mixer/strip-buttons (t) (label t.name :color :accent))

;; Wrap it: `original` is the factory version, to call or not.
(override eseq.mixer/strip-buttons :around (original t)
  (v-stack :gap 0.2 (label "custom") (original t)))
```

- The target is written qualified, `module/name`, and read as syntax.
- An override belongs to the module it is written in. Evaluating that module
  again replaces it; it never stacks. A reload of the target keeps it. When
  two modules override one function, the one evaluated last wins.
- One that throws is switched off and the factory version comes back.
- `(remove-override module/name)` drops it; `disable-override` and
  `enable-override` switch it; `(disable-module-overrides some.module)` switches
  off everything a package overrides.

### Changing one prop deep inside

A widget is a dict, so an `:around` can change what `original` returned
instead of copying a long function. Find the widget by its authored `:key`:

```lisp
(def recolor (w key)
  (if (and w (= (get w :key) key))
    (merge w :active-background-color :red)
    (if (and w (get w :children))
      (merge w :children (map |c| (recolor c key) (get w :children)))
      w)))

(override eseq.mixer/track-collapsed-strip :around (original t)
  (recolor (original t) (str "track-collapsed-mute-" t.index)))
```

`merge` takes keyword/value pairs, not a second dict.

## Colours

A colour is a theme slot keyword (`:red`, `:accent`, `:dim`, `:control-on-bg`,
`:mixer-control-bg`, …) or `(rgba r g b a)` with 0..1 parts. Prefer a slot: it
follows the user's theme. `(apply-theme (dict :accent '(0.95 0.25 0.65)))` changes
slots everywhere; unmentioned slots keep their values. Revert does not undo a
theme change; restarting the app does.

## Knobs, keys and hooks

- A factory module's `defcustom` is a knob: `(setopt eseq.mixer/track-strip-width 14)`.
  `apropos` with the area's name lists them. The user's Customize dialog keeps
  its own `setopt`s in a managed block at the end of init.lisp; leave that block
  alone.
- `(bind-key "C-c m" "user.init/focus-mixer")` binds a key to a command: any
  `def` of a declared module, by its qualified name.
- `(add-hook "hook-name" "my-entry-key" (lambda (…) …))` runs code at a factory
  extension point; the entry key keeps a reload from adding it twice.

Revert removes overrides only. A key binding or hook added by `try` stays until
the app restarts.

## Gotchas

- `(track 1).muted` does not parse. Bind it first:
  `(let ((t (track 1))) t.muted)`.
- Include the `(import …)` lines your forms need in every `try`; repeats are
  skipped.
- Mute and solo through `eval` change the headless copy only.
