# Making sequencers with an agent

The factory sequencers, **alez/neural** and **alez/jaki**, are packages written in eseq's own Lisp. Yours can be too. A sequencer module decides when notes play and which ones, keeps its settings in the project, and draws its own tab beside **Seq**, graphics included. A coding agent can write one from a description, check it with a tool eseq provides, and look at a picture of its panel. Nothing needs compiling and eseq does not need restarting.

This chapter follows one sequencer from a first request to a package someone else can install. It assumes the setup in [Making instruments with an agent](agent-authoring): a coding agent such as Claude Code or Codex, and eseq launched at least once.

## What a sequencer is made of

A custom sequencer defines a **kind**. Each sequencer you create from it is an **instance**, with its own tab, its own settings and its own place in a rack if a Drum Rack owns it. A kind has three parts:

- **Settings**, stored per scene and saved with the project. Undo and redo cover them, and duplicating an instance copies them.
- **A tick**, which eseq runs on every step (a sixteenth, by default) while the transport plays. It reads the settings and sends notes to tracks.
- **A panel**, built from the same widgets as eseq's own interface: number pickers, menus, editable lists, and shaders you write yourself for the graphics.

The source is usually two short files: one for the tick, one for the panel. They live in your Local package folder:

```
~/.eseq.d/packages/local/
```

A folder there becomes part of the module name: `euclid/rings.lisp` is the module `euclid.rings`. See [Packages](packages) for how Local, Installed and Factory modules relate.

## Your first sequencer

1. Choose **Help > Open Authoring Folder in Terminal** and start your agent there, for example by typing `claude` or `codex`. The authoring guide in that folder tells it where sequencers go and which rules to read.
2. Describe the sequencer. Say how it decides what to play, what you want to control, and what the panel should look like:

   > Make me a Euclidean sequencer as a Local package. Up to six rings, each with steps, hits, rotation, a track, note, velocity and probability. Draw the rings in a shader: each ring's hits as dots, a hand per ring showing its current step, and a glow in the middle when several rings hit together. Let me drag a ring to rotate it.

3. Let the agent work. It reads the sequencer rules, studies the factory Jaki package, writes the two files, and runs the check until it passes. It then opens the picture of the panel and fixes anything that overlaps or is cut off.
4. In eseq, open the **Packages** tab. Under **Local**, right-click the new module (the one with the panel, such as `rings`) and choose **New euclid**. The new tab opens beside **Seq**.
5. Press Play. If nothing sounds, look at the status line. A failing tick shows a message such as "Sequencer 'euclid 1' tick failed", followed by the error. The sequencer then stays silent until its files change. Tell the agent what you hear and what the status line says.

The check the agent runs cannot hear the sequencer. It confirms that the code loads and that the panel draws, but only you can say whether it plays the right notes, so listening is part of the loop.

Some requests that work well:

- "A Turing Machine style sequencer: an 8 or 16 step random loop with a lock knob that decides how often a step changes, plus a scale quantizer."
- "A probability grid: 16 steps by 4 tracks where each cell is a chance of playing, drawn as a heat map."
- "A bouncing-ball sequencer: balls fall at different speeds and play a note each time they hit the floor."
- "Like the Euclid rings, but each ring plays a chord."

## Changing a sequencer

Keep talking to the agent: "let me type a list of notes and loop through them", "allow more hits than steps so they ratchet", "add an Off choice to the track menu", "give the list columns more room". It edits the files and runs the check again.

eseq reloads a Local file as soon as it is saved, so changes appear in an open tab without a restart. Existing settings survive as long as the setting names stay the same. If a change leaves the module unable to load (for example, while the agent is halfway through editing two files), the sequencer may drop out of the rack menu until the files are fixed. Choosing **New euclid** again from the Packages tab brings it back.

Settings are part of the project, so an agent should add new settings rather than rename old ones. When a setting changes shape, say from a single number to a list, ask the agent to keep reading the old form too, so that a saved `4` still works where a list of hits is now expected.

## Racks

A Drum Rack can own an instance: right-click the rack's header and choose **New euclid in rack**. The instance's settings then travel with the rack's clips, and track numbers in the tick refer to the rack's pads. The menu lists a Local sequencer only once its module has been loaded in the current session, for example by creating one instance from the Packages tab. A packaged sequencer is always listed, because its manifest names the kind.

## What the check does

The agent tests its work with the `eseq` command-line tool. You can run it yourself:

```
/Applications/ESeq.app/Contents/MacOS/eseq sequencer check euclid.rings
```

| Stage | Fails when |
|---|---|
| module | no file has that module name, or the file does not begin with `(module …)` naming it |
| parse | a bracket is unbalanced in the module or in a Local or installed module it imports; the line is given |
| def-kind | the module defines no kind |
| panel | loading the module, creating an instance or drawing its tab fails |

On success it prints the path of a PNG of the panel. `--eval` runs a form after the instance is created, to fill it with example settings or to show a moment of playback in the picture. `--no-render` skips the panel. The last line of the report is a reminder that the tick was not run.

## Sharing a sequencer as a package

A Local module is for your own projects. To give a sequencer to someone else, ask the agent to turn it into a package:

> Turn the euclid sequencer into a package called alec/euclid, version 0.1.0, and install it.

The agent builds a package folder (a `manifest.json` that names the sequencer, and the modules renamed into the package's namespace), installs it, and checks the installed copy. The package then appears under **Installed** in the Packages tab. Zip the folder and send it; the recipient installs it with **File > Import Package…**. Packages carry code that runs with eseq's own access, so install them only from people you trust.

A packaged sequencer is a different kind from its Local original, so instances in projects made before the move keep using the Local files. Keep the Local folder until those projects have been rebuilt with the package version.

## Chat assistants without file access

An assistant in a chat window can draft a sequencer if you give it the rules. Paste the contents of `ESeq.app/Contents/Resources/authoring/sequencer-reference.md` with your request, save the files it writes under `~/.eseq.d/packages/local/<folder>/`, run the check command above, and paste any `FAIL` lines back into the chat.

## Going further

The rules file the agent reads, `sequencer-reference.md`, is also the quickest reference for writing a sequencer by hand. The factory packages are working examples: in the Packages tab, right-click a factory module and choose **View Source**, or **Copy to Local** to change a copy. For per-step processes and other building blocks, see [Packages](packages) and [Process lanes](process-lanes).
