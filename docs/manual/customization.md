# Keys and customization

eseq's interface is built from **buffers** shown in **tiles**, and every key press is routed by focus and by the active buffer's **mode**. The screens, menus, modes and most key bindings are defined in eseq's Lisp and loaded at startup; a small set of transport and editing shortcuts is built in (see Rebinding keys). This chapter explains how a key finds its target, lists the shortcuts, and then covers the ways to change eseq without writing a package: the Customize dialog, themes, `init.lisp`, the project scratch and key rebinding.

**Help > Keyboard Shortcuts** opens this page.

## Buffers, tiles and modes

A **buffer** is a named view. The session view is several buffers side by side (the screen map in the [eseq Manual](index) shows where each sits): `*transport*` along the top, `*samples*` (the browser), `*sequencer*`, `*step*` (the step inspector), `*track*` (track settings), `*mixer*`, and the device panel, which shows `*fx*` or `*piano-roll*`. `*arrangement*`, `*manual*` and the project's `*scratch*` are buffers too. Names with asterisks are eseq's own; a file opened for editing is a buffer named after the file.

A **tile** is an area of the window that shows one buffer. The layout you see is a set of tiles, and the **active tile** is the one that receives keys. Clicking in a tile makes it active.

A **mode** belongs to a buffer and carries that buffer's key bindings. Modes can inherit: the step grid, mixer, device panel, arrangement, piano roll, browser and transport all inherit one shared mode for the sequencer keys (arrows, Return, Backspace), then add keys of their own. The mixer's mode, for example, turns Left and Right into "previous and next channel".

## Where a key goes

A key press is offered to the following, in order, and the first one that accepts it keeps it:

1. **Undo and redo.** Command-Z and Command-Shift-Z undo and redo the last edit in eseq's views, unless a text field or the patch editor has focus. The patch editor keeps its own undo history, and in a text buffer they undo typing.
2. **eseq's built-in shortcuts** (the set is named under Rebinding keys). Apart from Command-G, these do not fire while a text field has focus.
3. **Space**, which starts and stops the transport in eseq's views, unless the focused control uses Space itself. In a text buffer such as `init.lisp`, Space types a space.
4. **The computer keyboard as an instrument.** When a track or rack is armed and no text field has focus, the note keys (A to L and the row above) play notes and `Z` and `X` shift the octave; other keys keep their bindings. Keys with Command, Control or Option never play notes. With roll mode on, the roll keys go to the roll even when no track is armed; see [Process lanes](process-lanes). [Recording](recording) shows the note layout.
5. **The focused control**: a text field, a number field being typed into, or a widget such as the patch editor.
6. **The active buffer's mode**: its key handler, then its bindings, then the bindings of the mode it inherits from.
7. **Global bindings**, made with `bind-key` (see Rebinding keys below). Period, the record toggle, is one.
8. **Built-in editing keys**, which apply in text buffers.

In practice this means three things. A text field takes your typing: in the browser's search box, letters are letters, and Period types a period instead of starting a recording. An armed track takes the note keys, so disarm it before using a single-letter shortcut on one of them. And a shortcut that does nothing usually means the wrong tile is active or something still has focus: press Escape, then click the panel you mean.

Note: a mode chooses whether the computer keyboard plays notes in its buffer. The factory views all allow it. A package view that declares its own mode without that option keeps its letter keys for itself.

## Shortcuts

Transport and recording:

- Space: play and stop
- Period: record on and off
- Semicolon: roll mode on and off (see [Process lanes](process-lanes))
- Command-R: arm the selected track and disarm every other; press again to disarm it
- Command-B: jump to the tempo field

Views and panels:

- Tab: switch between the session view and the arrangement
- Shift-Tab: open or close the piano roll in the device panel; in the instrument or effect editor, switch between the patch graph and its source
- Command-1 to Command-9: select a tab in the sequencer tile, counting from **Seq**
- Command-M: show or hide the selected track's instrument **mods** tab
- Command-F: jump to the browser's search field
- Control-Tab: move the piano roll back to the device panel at the bottom
- Control-H: collapse every expanded track row

Editing:

- Command-Z, Command-Shift-Z: undo, redo
- Command-A: select everything in the current surface, whether steps, piano-roll notes or arrangement clips
- Command-C, Command-V: copy and paste steps, notes or an arrangement region
- Escape: clear the selection
- Command-D: in the mixer, duplicate the selected pattern; in the arrangement, duplicate the selected region
- Command-G: group the selected tracks (two or more)
- Command-P: place mode in the arrangement
- Command-plus, Command-minus: double or halve the pattern length
- Command-I: create a new instrument in the patch editor
- Command-Option-I: open the source of the selected track's instrument UI (see [Instruments](instruments))

Files:

- Command-N: new project
- Command-O: open a project
- Command-S, Command-Shift-S: save, save as
- Command-Shift-E: export audio
- Command-Comma: Settings
- Command-Q: quit, with a prompt to save unsaved changes

The chapters on each area describe their own keys: the step grid in [Step sequencer](sequencer-tour), note entry in [Piano roll](piano-roll), placement and regions in [Arrangement](arrangement), and the computer-keyboard layout in [Recording](recording).

## The menu bar and Search Commands

**File** holds projects, audio export, sample and package import and export, Settings, Customize and the two Lisp files described below. **Edit** has the clipboard commands plus **Edit Selected Effect…** and **Edit Instrument…**. **Create** adds instruments, effects, buses, MIDI and sampler tracks, drum and layer racks, scenes and scene banks. **Pattern** acts on the selected track pattern: double or halve its length, clone it, capture MIDI or resample the last 30 seconds into it, shift its steps, transpose it or clear it.

The **View** menu shows or hides the browser, the mixer, the device panel (**Show Track FX**) and the patch macros panel. It also has **Collapse All Tracks** and **Restore Default Layout**. Restore Default Layout shows every panel, returns the device panel to the bottom and rebuilds the session view's tiles. It repairs any layout you have split or closed by accident.

**Help > Search Commands…** lists every enabled menu command, with its shortcut, in a prompt at the bottom of the window. Type part of a name to filter the list. Up, Down and Tab move through the matches, Return runs the selected one, and Escape cancels. It is the quickest way to find a command when you know its name but not its menu.

## Commands and chords

Beneath the menus, eseq keeps an Emacs-style command layer. In command names and bindings, `C-` means Control, `M-` means Option, `S-` means Shift and `s-` means Command. A space separates keys pressed in turn, so `C-x b` is Control-X followed by B.

`M-x` (Option-X) opens the command prompt. Every Lisp function eseq has loaded can be run from it, including the ones packages add. Type any part of a name and the first match appears in brackets. Tab moves to the next match, Return runs it, and Escape cancels. Qualified names such as `eseq.customize/customize` belong to a module; `seq-theme-aura` is a plain global.

The factory chords:

- `C-x b`: list the open buffers in the current tile. Type to filter, move with the arrows, press Return to switch. Escape clears the filter, and Escape again goes back. Buffers already shown in another tile are left out.
- `C-x 2`, `C-x 3`: split the active tile into two, one above the other or side by side
- `C-x o`: make the next tile active
- `C-x 0`: close the active tile; `C-x 1`: close every tile except the active one
- `C-x m`: show or hide the patch macros panel
- `C-x p`: open the packages view (see [Packages](packages))
- `C-c p`: open the sound palette
- `C-h m`: open this manual
- `C-x C-e`: evaluate the expression at the cursor, in a Lisp buffer
- `C-x C-b`: evaluate the whole buffer
- `C-x C-s`: save the buffer to its file

The tile commands change the session view's layout until you choose **View > Restore Default Layout**.

## This manual

![The manual’s navigation bar shows both clickable actions and their keyboard shortcuts.](images/manual-navigation.png)

**File > Help**, **Help > Documentation** or `C-h m` opens the manual in its own buffer. Links are clickable and the page scrolls. The navigation bar at the top has the same actions as the keys:

- `n` and `p`: next and previous chapter
- `u`: up to the page that listed this one
- `l`: back to the previous page
- `t`: the top page
- `g`: reload the page
- `q`: close the manual and return to the buffer you came from

## Customize

**File > Customize…** opens the Customize dialog. It lists every setting that eseq or an installed package declares as a **knob**, grouped by the module that owns it. It also lists every package override of a factory definition. `M-x eseq.customize/customize` opens the same dialog.

Each knob row shows the knob's name, a one-line description, an editor for its type and a **Reset** button. The editor is a number field, a switch, a text field, or a menu when the knob offers a fixed set of choices. A knob that differs from its default is marked *(customized)*. Changes apply as you make them, so a width knob resizes the mixer while you drag it.

The factory knobs control the layout:

- `eseq.mixer`: `track-strip-width` (11 to 24, default 12.9) and `bus-strip-width` (9 to 20, default 10.3), the widths of mixer strips
- `eseq.seq-core-state`: `mixer-show-clip-grid` (on), `mixer-clip-area-height` (2 to 12, default 4), and `corner-radius-scale` (0 for square corners to 1 for fully rounded)
- `eseq.seq-layout`: `transport-bar-height`, `samples-sidebar-ratio` (the browser's share of the window width, default 0.2), `macro-mapping-sidebar-ratio`, and `lower-panel-ratio` (default 0.33, see the note below)
- `eseq.seq-step-tabs`: `lower-fx-layout-height` (6 to 30, default 11.5), the device panel's height, and `seq-tile-border-width`
- `eseq.sequencer`: `step-cell-width` and `step-cell-height`, the size of each step in the grid

Sizes are in cells, eseq's layout unit.

Note: two knobs set the bottom panel's height. The device panel is held at `lower-fx-layout-height`, so change that one to make it taller or shorter. `lower-panel-ratio` applies when the piano roll is in the bottom panel: it sets the piano roll's share of the height, which never drops below `lower-fx-layout-height`.

For example, to get a compact mixer, find `mixer-show-clip-grid` under `eseq.seq-core-state` and turn it off. The pattern cells at the top of each strip disappear, and the strips and the mixer panel become shorter. Scenes still launch from the transport's scene buttons.

### Overrides

A package can replace a factory definition, such as the function that draws the mixer, with an `override`. The **Overrides** section lists these, grouped by the module that installed them. Each entry names the definition it replaces and whether it replaces it outright (**replace**) or wraps it (**around**).

The switch beside a module's name turns all of its overrides off or on; the switch beside an entry turns just that one. With a module switched off, the factory definitions are back in use at once and the module stays loaded, so it can be switched on again without reloading anything. An entry marked **quarantined** raised an error when it ran, and eseq has already fallen back to the factory definition for it.

### Saving

Changes in the dialog last until eseq quits. **Save** keeps them: eseq writes a managed block at the end of `~/.eseq.d/init.lisp`, with one line per knob that differs from its default, one per module whose overrides you switched off and one per single override switched off, and leaves the rest of the file alone. The block is added at the end of the file the first time and stays where it is afterwards. For the compact mixer above, the block reads:

```lisp
;; customize -- managed, edit via M-x customize
(setopt eseq.seq-core-state/mixer-show-clip-grid false)
;; end customize
```

A knob reset to its default drops out of the block, and when nothing is customized the block is removed. The header shows **unsaved changes** until you save. **Close** or Escape dismisses the dialog.

Package authors declare a knob with `defcustom` in their module. `:type` and `:doc` are required; `:min`, `:max` and `:step` set the number field's range and step, and `:choices` turns the editor into a menu:

```lisp
(defcustom rows 8 :type :number :min 1 :max 32 :step 1
  :doc "Rows shown in the pattern view.")
```

## Themes

eseq ships 14 color themes. Each is a command: run `M-x`, type `theme`, and pick one of:

- `seq-theme-mac-osx-dark` (the default), `seq-theme-mac-osx-light`, `seq-theme-mac-osx-graphite`, `seq-theme-mac-osx-haze`, `seq-theme-mac-osx-midnight`, `seq-theme-mac-osx-midnight-50`, `seq-theme-mac-osx-ember`, `seq-theme-mac-osx-violet`
- `seq-theme-ableton-mid`, `seq-theme-black-ir`, `seq-theme-tahoe-terminal`, `seq-theme-phosphor`, `seq-theme-phosphor-blue`, `seq-theme-aura`

The theme applies at once, and a message confirms its name. It is not remembered: eseq starts in the default theme. To keep one, add its command to `init.lisp`, for example `(seq-theme-aura)`.

## init.lisp and the project scratch

Two Lisp files belong to you rather than to eseq.

**init.lisp** is your personal setup, at `~/.eseq.d/init.lisp`. **File > Open init.lisp** opens it in a tab in the sequencer tile and creates it if it does not exist. eseq evaluates it after everything else at startup, so it can change anything the factory files set up. eseq also watches the file and evaluates it again when it changes on disk. This is where the Customize block, key bindings, a theme, MIDI controller mappings (see [Recording](recording)) and the `import` lines of packages you always load belong.

The **project scratch** belongs to one project. **File > Open Project Scratch** opens it in a **scratch** tab beside **Seq** (the buffer is `*scratch*`). It holds code that the project needs, typically the `(import …)` lines of the packages attached to it; attaching a package from the browser writes that line for you (see [Packages](packages)). Press `C-x C-b` to evaluate the whole buffer. The project saves both the text you are editing and the last version that evaluated without error, and it is the evaluated version that runs when the project is opened.

In either file, `C-x C-e` evaluates the single expression at the cursor, which is the fastest way to try a binding or a setting before keeping it. In the project scratch, only `C-x C-b` updates the version the project replays: an expression run with `C-x C-e` takes effect now but does not run the next time the project opens unless the whole buffer is evaluated.

## Rebinding keys

`bind-key` binds a key or chord to a command everywhere. `mode-bind-key` binds one inside a single mode. `bind-key` takes a key, in the notation above, and a command name. `mode-bind-key` takes a mode name, a key and a command name. All three are strings, and names from eseq's modules are written in full. Put them in `init.lisp`:

```lisp
;; Control-C then B shows or hides the browser.
(bind-key "C-c b" "eseq.seq-panels/seq-toggle-samples-sidebar")

;; In the mixer, Shift-Right also moves to the next channel.
(mode-bind-key "eseq.mixer/seq-mixer-mode" "S-RIGHT" "eseq.mixer/select-next-channel")
```

Because a mode's bindings come before the global ones, a global binding never overrides a key that a view already uses. To change a key inside a view, bind it in that view's mode. The shared sequencer keys live in the mode `eseq.sequencer-keys/sequencer-keys`, so rebinding a key there changes it in every view that inherits it. The factory views use these modes:

- the step grid: `eseq.seq-grid-mode/seq-grid-mode`
- the mixer: `eseq.mixer/seq-mixer-mode`
- the arrangement: `eseq.arrangement/arrangement-mode`
- this manual: `eseq.manual/manual-mode`
- the shared parent of the sequencer views: `eseq.sequencer-keys/sequencer-keys`

A package that adds a view declares its own mode with `define-mode`. `:inherit "eseq.sequencer-keys/sequencer-keys"` gives the view the standard sequencer keys, and `:live-keys true` lets the computer keyboard play notes in it.

Some keys are built in and handled before any Lisp binding, so `bind-key` cannot replace them: undo and redo, Space, Tab, Shift-Tab, Control-Tab, Control-H, and Command-R, -B, -F, -M, -G, -I, -Option-I, -A, -1 to -9, -plus and -minus, and the mixer's Command-D. Command-P and the arrangement's Command-D, -C and -V are ordinary bindings in `eseq.arrangement/arrangement-mode` and can be rebound in that mode. Period and Semicolon are global bindings, so `bind-key` can move them.

### Patch editor keys

The patch editor has its own table, changed with `eseq.patcher/bind-key` and `eseq.patcher/unbind-key`. In this table `P-` stands for Command. The commands are:

- `create-below` (`P-RET`) and `connect-last-two` (`P-UP`)
- `undo` (`P-z`), `redo` (`P-S-z`), `copy` (`P-c`), `paste` (`P-v`)
- `encapsulate` (`P-e`) and `toggle-cable-style` (`P-y`)
- `open-macro` (`RET`) and `delete-selection` (Backspace or Delete)
- `open-bubble` (`P-k`), `connect-bubble` (`P-S-k`), `retry-bubble` (`P-r`) and `dismiss-bubble` (`ESC`), for the agent prompt
- `accept-suggestions` (`Tab`), for suggested cables

For example, `(eseq.patcher/unbind-key "Tab")` stops Tab from accepting suggested cables.

## Settings

**File > Settings…** (Command-Comma) lists MIDI inputs and lets you enable or disable each one. [Recording](recording) describes it with the rest of MIDI input.

To go further than settings and bindings, with new sequencers, views or effects, see [Packages](packages).
