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

**File > Help** opens it. Links are clickable and pages scroll.

- `n` and `p`: next and previous chapter
- `u`: up to the parent page
- `l`: back
- `t`: top page
- `q`: close the manual
