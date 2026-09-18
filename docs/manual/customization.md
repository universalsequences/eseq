# Navigation and keys

Panels in eseq are **buffers** and the areas that show them are **tiles**. The view controls handle the common layouts; the keyboard reaches everything else.

## Focus

A key does what the focused panel says. In a search box, letters type. On an armed track, letters play notes. In the piano roll, Delete removes notes. When a shortcut does nothing, leave the text field, disarm the track, and click the panel you mean.

## Shortcuts

- Space: play and stop
- Period: record
- Tab: session and arrangement
- Shift-Tab: devices and piano roll
- Command-Z, Command-Shift-Z: undo, redo
- Command-D: duplicate the pattern in the mixer, or the selection in the arrangement
- Command-G: group the selected tracks
- Command-P: place a pattern in the arrangement

## Commands and buffers

In command help, `C-` is Control, `M-` is Option, `S-` is Shift, and `s-` is Command. A space between chords means press them in turn.

`C-x b` switches the current tile to another buffer. `M-x` opens the command prompt; type a command name and press Return. The music buffers are `*sequencer*`, `*mixer*`, `*fx*`, `*piano-roll*`, and `*arrangement*`.

## This manual

![The manual’s navigation bar shows both clickable actions and their keyboard shortcuts.](images/manual-navigation.png)

**File > Help** opens it. Links are clickable and pages scroll.

- `n` and `p`: next and previous chapter
- `u`: up to the parent page
- `l`: back
- `t`: top page
- `q`: close the manual

## Customize

`M-x eseq.customize/customize` opens the Customize dialog, a one-stop list of every setting a module or package declares as a knob, grouped by the module that owns it. Each row shows the knob's name and description and an editor for its type: a number field, a switch, a text field, or a dropdown when the knob offers a fixed set of choices. Changes apply immediately, so you can watch the mixer resize as you turn a width knob. For example, turning off `mixer-show-clip-grid` under `eseq.seq-core-state` hides the clip launch grid and gives you a compact mixer with shorter strips and a shorter mixer panel. **Reset** returns a knob to its factory default, and a knob that differs from its default is marked *(customized)*.

The **Overrides** section below the knobs lists every factory definition a package has replaced, grouped by the package that replaced it. Turn a package's switch off to get the factory behavior back without uninstalling the package; turn it on again to restore the package's version. No reload is needed in either direction. A row marked *quarantined* errored when it ran, so eseq is already using the factory definition for it.

Press **Save** to keep your changes across sessions. eseq writes them into a managed block at the end of `~/.eseq.d/init.lisp`, one `(setopt …)` per changed knob and one `(disable-module-overrides …)` per package you turned off, and leaves the rest of that file alone. A knob you reset to its default is removed from the block. **Close** or Escape dismisses the dialog.

Package authors add a knob with `(defcustom name default :type :number :min 0 :max 10 :step 0.5 :doc "…")` in their module; the range is optional but keeps the number field's drag sensible. See [Packages](packages).
