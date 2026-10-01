# Making instruments with an agent

Every compiled instrument and DGenLisp effect in eseq is two short text files: `dsp.lisp`, the signal processing, and `ui.lisp`, the panel. A coding agent, an AI assistant that works in a terminal and can read and write files, can write both for you. You describe the sound; the agent writes the files into your library, tests them with a checker eseq provides, and looks at a picture of the panel it built. The result behaves like any factory instrument: it has presets, parameter locks and modulation, and it can go in a rack or be shared as a package.

This is the quickest way to get a new sound. The patch editor described in [Instruments and presets](instruments) remains for building instruments by hand.

## What you need

- A coding agent that runs in the terminal, such as Claude Code or Codex, installed and signed in according to its own instructions. It uses your own account with that service; eseq needs no API key for this.
- eseq launched at least once. The first launch writes the authoring guide the agent reads.

## The authoring folder

Your own instruments, effects, projects and samples live in one folder:

```
~/Library/Application Support/com.universalsequences.eseq/
```

**Help > Open Authoring Folder in Terminal** opens a Terminal window there. The parts that matter here:

- `instruments/`: your instruments, one folder each. They appear in the **Library** section of the browser's Instruments tab.
- `effects/`: your DGenLisp effects. They appear under **Custom** in the Audio FX tab.
- `AGENTS.md`: the authoring guide. It tells the agent where to write, which rules to follow, which factory instruments to study and how to test its work. Codex and most other agents read `AGENTS.md` by themselves.
- `CLAUDE.md` and `.claude/skills/`: the same guide, in the places Claude Code looks for it.

eseq rewrites these three guide files each time it starts, so their paths and rules stay current after an update. To keep your own edits to one of them, delete its first line (the one beginning `<!-- eseq:generated`); eseq then leaves that file alone.

The guide points the agent at references inside the application: the instrument and effect rules and the DGenLisp language reference in `ESeq.app/Contents/Resources/authoring/`, and the factory instruments and effects as read-only examples.

## Your first instrument

1. Choose **Help > Open Authoring Folder in Terminal**.
2. Start your agent in that window, for example by typing `claude` or `codex` and pressing Return.
3. Describe what you want. Say what it should sound like and which controls you want to reach for:

   > Make me a warm polyphonic pad: two detuned saws and a sub, a ladder filter with its own envelope, slow LFO on the filter, and a chorus-ish stereo spread.

4. Let the agent work. It reads the guide, studies one or two factory instruments, writes `instruments/<Name>/dsp.lisp` and `ui.lisp`, and runs the checker until the instrument passes. It then opens the picture of the panel and corrects the layout if controls overlap or are cut off. It may ask for permission before running commands, depending on how your agent is set up.
5. In eseq, open the **Instruments** tab, find the new instrument under **Library**, and double-click it to put it on the selected track, or drag it onto a track. eseq notices new folders while it runs, so there is no need to restart it. If something written outside eseq does not appear, **Edit > Rescan Instruments & Effects** reads the folders again. Automatic pickup can be turned off with the **reload-lisp-on-change** option in Customize; see [Keys and customization](customization).

Some requests that work well:

- "An 808-style kick with a pitch envelope, click and drive knobs."
- "A plucked string using a waveguide, with brightness, damping and body controls."
- "A two-operator FM bass with a filter and velocity to FM amount."
- "Like PM Flute but breathier, with a growl control."

Naming a factory instrument as a starting point is useful: the agent can read its source.

## Changing an instrument

Keep talking to the agent: "make the filter envelope snappier", "add a noise layer", "the panel is too crowded, split it into two columns". It edits the files in place and runs the check again.

Changes to the panel (`ui.lisp`) appear as soon as the file is saved. A change to the sound (`dsp.lisp`) of an instrument that is already on a track needs one step: select the track and choose **Edit > Reload Instrument From Disk**. For an effect, click its header in the device panel and choose **Edit > Reload Selected Effect From Disk**. The reload keeps the current value of every parameter whose name is unchanged, and your parameter locks. If the new version does not compile, the status line says so and the previous sound keeps playing. Double-clicking the instrument in the browser also loads the new version, but as a replacement: it resets the knobs and clears the instrument's parameter locks.

The agent keeps parameter names where it can, because projects store knob values and parameter locks by name. If a parameter is renamed, locks on the old name no longer apply.

## Effects

Effects work the same way. Ask for one ("a tape delay with wow, flutter and a saturating feedback path"), and it appears in the Audio FX tab under **Custom**. Drag it onto a track's effect chain. Reloading an edited effect is described under Changing an instrument above.

## What the check does

The agent tests its work with the `eseq` command-line tool inside the application. You can run it yourself from the authoring folder:

```
/Applications/ESeq.app/Contents/MacOS/eseq instrument check "instruments/Warm Pad"
/Applications/ESeq.app/Contents/MacOS/eseq effect check "effects/Tape Echo"
```

Each stage prints a line. A failure stops the check with the stage and the reason, and the command exits with an error:

| Stage | Fails when |
|---|---|
| compile | the DSP does not compile |
| ui.lisp | the panel has no `defsynth-ui` or `defeffect-ui` form, names a parameter the DSP does not have, or uses an outdated helper name |
| audition | a test note (or, for an effect, a test signal) comes out silent or clips |
| panel | the panel fails to draw in eseq's own renderer |

Lines marked `warn` point out conventions (an effect that leaves the test signal unchanged, for example) without failing the check. When the panel draws, the check prints the path of a PNG of it. A panel failure that says eseq's own UI failed to load points to a problem with the installation rather than with the instrument; `--no-render` runs every stage except the panel. `eseq paths` prints the folders the checker and the guide use.

## Using the skill from any folder

Claude Code picks up the guide only when it is started in the authoring folder. To make it available wherever you start Claude Code, install the skill in your home folder:

```
mkdir -p ~/.claude/skills/eseq-authoring
/Applications/ESeq.app/Contents/MacOS/eseq authoring skill > ~/.claude/skills/eseq-authoring/SKILL.md
```

The skill points back to the guide in the authoring folder, so it follows eseq updates.

## Chat assistants without file access

An assistant in a chat window cannot write files or run the check, but it can still draft an instrument. Give it the contents of `ESeq.app/Contents/Resources/authoring/instrument-reference.md` (or `effect-reference.md`) along with your request. Save its two code blocks as `dsp.lisp` and `ui.lisp` in a new folder under `instruments/`, run the check command above, and paste any `FAIL` output back into the chat.

## Agent Mode inside eseq

eseq also has a built-in conversation buffer for drafting instruments and effects, **Agent Mode** (`C-x a`). It follows the same rules but calls a model provider directly, so it needs an API key in an environment variable such as `ANTHROPIC_API_KEY`. The terminal route above uses your agent's own account instead.

## Sharing what you make

Instruments and effects you make can be shared as a package: **File > Export Package…** bundles chosen instruments and effects into a file someone else can install. See [Packages](packages).

The same agent can also write sequencers, which are Lisp packages rather than DGenLisp; see [Making sequencers with an agent](sequencer-authoring).
