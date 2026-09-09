# Navigation and controls

eseq's panels are called **buffers**, and the screen areas displaying them are **tiles**. You can use ordinary view controls without learning the buffer system. Keyboard commands provide another route to the same tools.

## Focus first

Click the panel you intend to operate before using a shortcut. A browser search needs letters for text; an armed track needs them for notes; a piano roll needs deletion to affect its note selection. The same key can mean different things in these contexts.

When a shortcut appears not to work, leave the text field, check whether a track is armed, and click the intended music panel.

## Common macOS gestures

- Space toggles playback in a music view.
- The red transport control toggles Record; period is also a record shortcut in appropriate music contexts.
- Tab switches session and arrangement views.
- Shift-Tab switches the lower main/device panel and piano roll.
- Command-Z undoes; Command-Shift-Z redoes.
- Command-G groups two or more selected tracks.
- Command-D duplicates the active track pattern in the mixer or the selection in the arrangement.
- Command-P starts pattern placement in the arrangement.

The track's R button arms input. It is not the same as pressing the letter R, which can be an editor shortcut. Use visible controls while learning.

## Buffer and command notation

In command help, `C-` means Control, `M-` means Meta/Option, `S-` means Shift, and `s-` means Super, which is Command on macOS. A space between chords means press them in sequence.

`C-x b` opens buffer switching: press Control-X, then B, and choose a buffer. `M-x` opens the command prompt: press Option-X, type a command name, and confirm. Named commands do not require writing code.

Music buffers include `*sequencer*`, `*mixer*`, `*fx*`, `*piano-roll*`, and `*arrangement*`. Prefer view controls to restore the normal multi-panel layout; switching one tile's buffer changes only that tile.

## Read the manual

Open Help from the File menu. Links are clickable and long pages scroll. Inside the manual:

- `n` goes to the next chapter in the parent menu.
- `p` goes to the previous chapter.
- `u` goes up to the parent page.
- `l` goes back through reading history.
- `t` returns to the top page.
- `q` leaves the manual and restores the previous buffer.

Chapter order comes from the menu. Return to [eseq Manual](index) to choose another topic.
