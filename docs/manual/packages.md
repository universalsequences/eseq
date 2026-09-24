# Packages

A **package** is a named, versioned bundle of eseq content that can be installed on any machine. It can carry Lisp modules, instruments, audio effects, preset banks and samples, in any combination. Packages are how eseq is extended. Graph sequencers, text-based rhythm languages, extra editing views and new per-step processes all arrive this way, and so do sound packs.

Five packages ship with eseq: the **alez/neural** graph sequencer, the **alez/tracker** tracker view, the **alez/jaki** rhythm language, **alez/sig** signal channels, and the **universalsequences/factory-samples** sample library. This chapter describes what a package holds and where packages live. It then covers the Packages tab, installing and exporting packages, and each factory package in turn. It ends with the building blocks for writing your own.

## What a package holds

A package is a folder. At its root is `manifest.json`, which gives the package's **identity** in the form `author/name`, such as `alez/neural`, and its version. The manifest can also name an **entry module**, the module loaded when the package is attached, and list other packages it depends on.

Beside the manifest, a package can have any of these folders:

- `src/`: Lisp modules. Each file declares a module in the package's own namespace, which is the identity with a dot in place of the slash. In `alez/tracker`, `src/ui.lisp` is the module `alez.tracker.ui`.
- `instruments/` and `effects/`: DGenLisp instruments and audio effects, laid out as in your own library.
- `presets/`: preset banks, including banks for factory instruments.
- `samples/`: audio files, usually indexed by a `samples.jsonl` file.
- `midi-fx/` and `themes/`: MIDI effects and colour themes. The import dialog counts them, but this version does not yet load either from a package.

A package can claim only its own namespace. The `eseq.` prefix is reserved, and a name without an author, such as `drums`, is refused.

Content and code behave differently. Once a package is installed, its instruments, effects, presets and samples appear in the browser straight away. Its Lisp modules do nothing until you **attach** them, either to a project or to every session. This is covered under the Packages tab below. A sound pack therefore needs no Lisp, and a sequencer package does nothing until you ask for it.

## Where packages live

eseq looks for Lisp modules in three **tiers**, in this order:

1. **Local**: `~/.eseq.d/packages/local/`. This is your personal workspace of modules. It has no manifest and is not a distributable package.
2. **Installed**: `~/.eseq.d/packages/`. There is one folder per package, named after its namespace, for example `~/.eseq.d/packages/alec.acid-tools/`.
3. **Factory**: the packages built into the app.

A module in Local shadows a module of the same name further down the list. Copy to Local, described below, relies on this.

There is no Uninstall command. To remove an installed package, quit eseq, delete its folder from `~/.eseq.d/packages/`, and start eseq again.

## The Packages tab

The **Packages** tab in the browser lists every module eseq can load. It has four sections:

- **Loaded**: every module the current project or your `init.lisp` imports, in alphabetical order. This is what is running now. A module imported by both shows the detail `project + always`.
- **Local**: your own modules, in folders.
- **Installed**: packages you have imported, each with its version, and their modules.
- **Factory**: the built-in packages.

A check mark after a module means it is attached to this project. A bookmark means it is loaded in every session. When both apply, the check mark is shown. Selecting a row shows its module name and attachment state in the status line. The search field at the top filters every section.

Press Return or double-click a module to attach it to the project. A package row attaches the package's entry module. A file without a `(module …)` header cannot be attached; the status line says so. Right-click a row for the full set of actions:

- **Attach to Project** / **Remove from Project**: add or remove the module's `(import …)` line in the project scratch.
- **Always Load** / **Stop Always Loading**: add or remove the same line in `~/.eseq.d/init.lisp`.
- **Edit Source** (Local) or **View Source** (Installed and Factory): open the file; see below.
- **Copy to Local** (Installed and Factory modules): copy the file into your Local workspace and open the copy for editing.

**New Package** creates a new module in your Local workspace from a template and opens it. Type a dotted name such as `my.euclid.sparse`. A bare name such as `euclid` becomes `my.euclid`, and a name ending in `/` creates a folder. Despite the button's name, the result is a module and not a distributable package; see "Writing your own" below. **Refresh** re-reads the tiers from disk.

### Project or every session

Attaching and Always Load write the same line, `(import module-name)`, to different files:

- **Attach to Project** writes it to the **project scratch**, which is saved in the project file. Anyone who opens the project, on a machine with the package installed, gets the module. This is the right choice for a sequencer the music depends on.
- **Always Load** writes it to `~/.eseq.d/init.lisp`, which runs at every start. This is the right choice for a view or a tool you want in every project, such as the tracker.

Attach to Project loads the module at once and writes the line only if it loads without error, so a broken module cannot damage the project. Always Load only writes the line: the module loads at the next start and is not checked now. To have it in this session as well, attach it to the project too.

Removing a module takes effect in two stages. Its replacements of factory definitions are switched off at once, so a replaced mixer, for example, reverts immediately. Other things it defined, such as its tab, may remain until you quit, because eseq cannot unload a module. The next time the project opens, the module is not loaded.

### Source tabs and Copy to Local

View Source and Edit Source open the module in a closable tab next to **Seq**. Installed and factory sources open read-only: a reinstall or an app update would overwrite edits.

To change an installed or factory module, use **Copy to Local**. eseq copies the file into `~/.eseq.d/packages/local/`, under the path that its module name maps to, and opens the copy for editing. Local comes first on the load path, so from then on every `(import …)` of that module loads your copy. Delete the copy to go back to the original.

### The C-x p view

`C-x p` opens a text view of your Local workspace in place of the sequencer. It is quicker than the browser for creating modules from the keyboard:

- Type a name to filter the list. Return on a name that does not exist creates the module or folder, and a preview line shows where it will go.
- Up and Down move the selection, and Return opens a file or enters a folder. `-` goes up one folder while the filter is empty.
- `C-a` attaches the selected module to the project. `C-i` adds it to `init.lisp`. `C-j` opens `init.lisp`.
- `q` (while the filter is empty) or Escape closes the view. `C-g` refreshes it.

In the list, ✓ marks a module attached to the project and ★ one in `init.lisp`.

## Installing a package

1. Choose **File > Import Package…**.
2. Choose a package folder, or a `.eseqpack` or `.zip` archive of one. A folder that is already inside `~/.eseq.d/packages/` is refused. An archive whose contents are wrapped in a single top-level folder is unwrapped. The package needs `manifest.json` at its root; if it has none, the import is refused and nothing is written.
3. Read the summary. It shows the identity, version and source path, and counts what the package carries: Lisp modules, instruments, effects, MIDI effects, preset banks, samples and themes. Only non-zero counts are listed.
4. Click **Install**. If a package with the same identity is already installed, the dialog says so and the button reads **Replace**. Replacing swaps the package folder. Presets you saved for its instruments are kept, because they live in your Library. Lisp modules already loaded in this session keep running the old code until you restart.

The package takes effect without a restart. Its instruments appear in the Instruments tab under a heading with the package's identity, and the **Packages** chip above that list shows packages only. Its effects appear in the Audio FX tab, its samples in the Samples tab with the package's dotted namespace, such as `alec.acid-tools`, as an origin chip, and its modules under **Installed** in the Packages tab. See [Instruments](instruments) and [Samples and sounds](sample-browser).

Packages are trusted code. eseq does not sandbox them: their Lisp runs with the same access as eseq's own interface, and their instruments compile to native code. Install packages only from people you trust.

Note: the import dialog warns that installing runs the package's Lisp. That Lisp runs once you attach a module; installing alone runs nothing.

## Exporting a package

**File > Export Package…** writes instruments, effects and presets from your Library into a package archive.

1. Enter a **package name** in the form `author/name`, such as `alec/acid-tools`, and a **version**. The version starts at `1.0`.
2. Switch on the items to include. There are three columns: **Instruments** and **Effects** from your Library, and **Presets**, which lists your saved preset banks for factory instruments. The count of selected items is shown at the bottom.
3. Click **Export…** and choose where to save. The archive is named `author.name-version.eseqpack`, for example `alec.acid-tools-1.0.eseqpack`.

The recipient installs the archive with **File > Import Package…**.

The export is designed to work on another machine:

- Presets saved for an exported instrument travel with it.
- Library macros that an instrument uses are copied into its source, so it compiles on a machine without your macro library. If a macro cannot be copied in, the export reports it.
- An absolute file path in an instrument's source is reported, because that file will not exist elsewhere. Move the file into the instrument's folder and refer to it by a relative path, then export again.
- Compile caches and audition files are left out.

Only Library items can be exported. To share a factory instrument or effect, **Fork** it first so a copy lands in your Library; see [Instruments](instruments) and [Audio effects](effects). The dialog does not export Lisp modules or samples. A package that carries code is assembled by hand, as described at the end of this chapter.

## Package ids

A project refers to an instrument or effect by a **tier-qualified id**. An instrument from a package is `pkg:author.name/` followed by its path inside the package. For example, `kits/808` from `alec/acid-tools` is `pkg:alec.acid-tools/kits/808`. Factory, Library and package items with the same name are therefore three different items. Installing a package never changes the sound of an existing project, and a project that uses a package's instrument needs that package installed to play it.

## The factory packages

The factory packages are listed under **Factory** in the Packages tab. None of them is attached to a new project; attach the ones you want. **alez/neural** is the most complete. **alez/tracker**, **alez/jaki** and **alez/sig** are experimental: they work, but their controls and syntax may still change.

### alez/neural: the graph sequencer

A **graph sequencer** makes notes from a network of **nodes** connected by weighted **edges**. The pattern is not written down anywhere; it emerges from how the network is wired and what sets it off.

The module is `alez.neural.variable-reset`, and it defines a sequencer **kind** called **neural**. Attaching the package creates nothing; each sequencer you want is an **instance** of the kind. Double-click the module row in the Packages tab (or pick **New neural** from its menu) to create one, or pick **New neural in rack** from a rack's menu to create one the rack owns. Each instance gets its own tab next to **Seq**, labelled with its name (**neural 1**, **neural 2**, …), and its own weights, routes and settings; a project or a rack can hold as many as you like. Rename, duplicate, move or delete an instance from its row in the Packages tab. The network has 8 nodes by default and up to 16. Every node begins routed to Track 1, so every note it makes plays on that track's instrument.

What happens, in outline, at each step boundary of a node's grid:

1. Each node gathers energy from the nodes that fired into it. An edge delivers its weight less its current dampening, and a node's stored energy leaks slowly on every step.
2. A node whose energy reaches the **threshold** fires. It emits a note on its routed track, and its incoming edges are dampened.
3. After the node's **delay**, the fired signal travels along its outgoing edges to other nodes, carrying the note with it. Each node on the way can transpose the note and scale its velocity.
4. If more nodes fire at one boundary than **max poly** allows, the **poly mode** decides which fires sound.
5. Every **reset bars** the network's state is cleared, and the cycle starts again.

A network needs something to set it off. A node with **seed rt** on listens to its routed track: a step on that track charges the node, and the step's note becomes the note the node passes on. A node with **rst seed** on starts each reset cycle charged.

The configuration block across the top holds the network-wide settings:

- **nodes**: 1 to 16.
- **reset bars**: the reset interval, 0 to 64; the default is 4. At 0 the network never resets.
- **max poly**: the most fires that sound at one boundary, 0 to 16; the default is 4. At 0 there is no limit.
- **poly mode**: which fires survive when there are too many; see below.
- **threshold**: the energy a node needs to fire, 0 to 4; the default is 0.55.
- **global trn**: a transpose, in semitones, added to every emitted note.
- **dur x**: note length as a multiple of the node's delay; the default is 1.

The poly modes are:

- **propagation**, the default: the fires whose signal will push the most neighbours over their threshold win.
- **deterministic**: a fixed order decides.
- **random**: the survivors are drawn at random.
- **markov**: a random draw, weighted by the edges from the nodes that won the previous boundary.
- **loudest**: the highest-velocity fires win.
- **lowest-transpose** and **highest-transpose**: the lowest or highest notes win.
- **seed-first**: fires set off by a seed win before fires that came only from the network.

Below that is one row per node:

- **route**: the track the node plays, or Off.
- **grp**: the node's group, A to D.
- **seed rt** and **rst seed**: as described above.
- **delay**: how long the node's signal takes to reach its neighbours.
- **transp** and **trn rst**: the transpose the node adds to the note it passes on. With **trn rst** off, transposes accumulate around a feedback loop, so a figure climbs on each lap. With it on, the node always plays its own transpose.
- **vel x** and **vel rst**: the velocity multiplier per hop. With **vel rst** on, the node always fires at full velocity.
- **dampen** and **recover**: how much a fire weakens the node's incoming edges, and how quickly they come back.
- **res** and **quant**: the node's timing grid, and the grid its fires snap to.
- **edit**: opens the node's full editor.

Beside the rows, two meters show each node's fires and its current energy; the energy meter is the quickest way to see how close a node is to its threshold. Next to them is the **weight matrix**, one cell per edge from a row's node to a column's node. Drag a cell to set that weight between 0 and 1. The engine accepts weights from −1 to 1, and a negative weight inhibits the node it points at, but a negative weight has to be set from Lisp, for example `(graph-edge (instance-ref 1) :from 0 :to 1 :weight -0.5)`, where 1 is the instance's id. The top block also shows the current edge dampening, a 3D history of recent fires and a spectrogram of the master output, and a keyboard along the bottom shows each track's sounding notes.

A new instance starts as a working network: its nodes are wired into a ring, each feeding the next at full weight, and node 0 has **seed rt** on. A duplicate keeps its source's settings instead.

1. Load an instrument on Track 1, and create a **neural** instance.
2. Every node plays on Track 1, and node 0 listens to it. To write the ring again later, run `(alez.neural.variable-reset/gvr-init-ring-defaults (instance-ref 1))` once, with the instance's id; do not put it in the project scratch, which runs every time the project opens.
3. Put one step on Track 1 and press Play. The step charges node 0, and its note travels round the ring one node per delay, a sixteenth at a time. Each hop scales the velocity by that node's **vel x**, 0.9 by default, so the note fades as it goes.
4. On node 0, set **transp** to 7 and turn **vel rst** on, leaving **trn rst** off. Each time the note comes round to node 0, it rises a fifth and returns to full velocity, so the figure climbs lap by lap until a fresh seed or the reset starts it again.

The node editor (**edit**) shows one node's row, its incoming and outgoing edges as two strips, and its **process patch**. The patch is a chain of the same processes that process lanes use. It runs each time the node fires, before its note is emitted and passed on. Transpose and velocity writes change the note that travels on. A veto silences the node's own note, but the fire still propagates, so **rand** into **cmp A** into **veto** makes the node sound only some of the time without breaking the chain. A process's source menu lists the nodes (nrn 0, nrn 1, …) after the tracks, so a **harmony** process can follow another neuron's chord instead of a track's. A process output's **map** button binds it to a field of the note: press **map**, then click the node's **delay**, **transp** or **vel x** field. Add a process with **+ add process**; cables are patched as in a track's patch bay. **all nodes** returns to the row grid. See [Process lanes](process-lanes) for the processes themselves.

A graph sequencer's settings are stored in the scene, so each scene can hold a different network: a sparse ring in the verse and a dense, self-exciting web in the chorus. A Drum Rack can also take ownership of the sequencer. Its routes then address the rack's members, the tab takes the rack's name, and the settings are stored in the rack's clips; see [Racks](racks).

### alez/tracker: the tracker view

The module `alez.tracker.ui` adds a **Tracker** tab beside **Seq**. It shows the same patterns as a tracker: one column per track and one row per step, up to 64 rows; steps beyond 64 are not shown. Each cell reads as a note name and a two-digit hex velocity, and `---` marks an empty step. A track shorter than the longest pattern repeats down its column in dimmed rows, and editing a dimmed row edits the real step.

Columns for parameters that already have locks appear automatically after the note and velocity. The **+** picker adds a column for any lockable parameter, or for a process lane. The tab edits the same data as the step grid and the inspector; it is another view of the pattern, not a separate pattern.

Click a cell or use the arrow keys to move. Return toggles a step, and Backspace clears it. The keys `a w s e d f t g y h u j k o l p` enter notes in the same layout as musical typing, and `z` and `x` shift the octave. On a velocity or lock column, type the value as hex digits.

Note: while a track is record-armed, the note keys belong to the live keyboard and record into that track instead of entering notes in the tracker.

The tracker is a good candidate for **Always Load**, together with Attach to Project if you want the tab in the current session. `C-c t` shows the tab once the module is loaded.

### alez/jaki: a rhythm language

Jaki writes a sequencer as one line of text. In the project scratch:

```lisp
(import alez.jaki.surface :refer (jak))

(jak "kit" :16
  . . - . (every 2 swap)
  -> 0
  -> 1 left
  -> 2 (shift 1) stac)
```

Evaluate the buffer with `C-x C-b`. `"kit"` names the sequencer; evaluating a `jak` with the same name replaces it. `:16` sets one unit of the grid to a sixteenth.

The part before the first `->` is the **figure**. A `.` is a single stroke and takes one unit. A `-` is a double stroke by the same hand and takes two units. Strokes alternate hands. Transforms such as `(every 2 swap)` change the figure from cycle to cycle.

Each `->` starts a **route**: a track number, counted from 0 for the first track, followed by route words. With no route, the figure plays on track 0. In the example, track 0 plays the whole figure, track 1 plays only the left-hand strokes, and track 2 plays it shifted one unit later, wrapping round the cycle, with shortened notes. Other route words include `right`, `accent`, `rev`, `ghost`, `(rot n)`, `(fast n)`, `(slow n)`, `(vel s)` and `(note n)`. A route that contains `(mute T)` or `(solo T)` plays no notes; it opens and closes a mute or solo on track T in time with the figure, so `-> (mute 3)` gates track 3 with the rhythm.

The route words are listed in the package's `core.lisp`, in the section headed "tier-2 route surface" (search for "Route words"), and the `jak` form is described at the top of `surface.lisp`. Use View Source.

### alez/sig: signal channels

`sig` defines a named **channel** whose value follows a periodic shape locked to the transport:

```lisp
(import alez.sig.surface :refer (sig))

(sig "sweep" :over (bars 4) (tri phase))
```

`phase` rises from 0 to 1 over each cycle, here four bars. The shapers `sine`, `tri`, `saw` and `sqr` turn it into a curve, and `scale` maps it to a range. A `sig` starts running as soon as it is evaluated. It updates once per `:rate`, a thirty-second note by default, and `:from` offsets its phase.

The value is derived from the transport position, not accumulated, so it stays in phase after a seek or a restart. Every `sig` with the same length is phase-locked to every other.

A jaki argument can follow the channel by reading `(chan "sweep")` in place of a number. With an instrument on Track 1, add a jaki sequencer below the `sig` and evaluate the buffer:

```lisp
(import alez.jaki.surface :refer (jak))

(jak "pulse" :16
  . . . .
  -> 0 (vel (chan "sweep")))
```

Track 1 plays straight sixteenths, and their velocity swells and falls over four bars.

### universalsequences/factory-samples

This package is the factory sample library: 76 samples from the TR-808 Sound Sample Set, the Salamander Grand Piano and the Versilian Community Sample Library. It is loaded at startup without being attached, and its samples carry the **Factory** origin in the Samples tab. [Samples and sounds](sample-browser) describes the collections and their licences. Its module, `universalsequences.factory-samples.library`, gives scripts a table from sample title to sample reference and is not needed for normal use.

## Writing your own

A module is a Lisp file that begins with `(module name)`, declares what it offers with `(export …)`, and uses other modules with `(import …)`. **New Package** in the Packages tab creates one from a template. Everything eseq's interface does is written this way, so a module can reach the same building blocks:

- `def-sequencer` defines a sequencer. A sequencer either runs a tick function on a grid and emits notes to tracks, or declares a graph of nodes and edges, as alez/neural does. A graph sequencer's settings are stored per scene.
- `def-process` defines a per-step process. Once loaded, it appears in **Add a lane** after the built-in lanes, and in a graph node's process patch. The built-in lanes are written with it; their source is `content/processes/builtin.lisp`.
- `defscene` declares a value stored per scene. A sequencer's panel can keep its settings there, so that switching scenes switches them.
- `defcustom` declares a setting that appears in **File > Customize…**; see [Keys and customization](customization).
- `override` replaces or wraps a factory definition, such as a mixer strip. It is listed under **Overrides** in Customize, where it can be switched off without removing the module.
- A module can add a tab beside **Seq**, as the tracker and the graph sequencer do.

The factory packages are the working reference for all of these. Open them with View Source, or use Copy to Local and change them.

A Local module is enough for your own projects. To share one, build a package folder by hand: a `manifest.json` and the module files under `src/`, renamed into the package's namespace. The importer checks these rules:

- `name` is the identity, `author/name`, and `version` is required.
- `entry`, if given, must be a module inside the package's namespace.
- `deps` lists the identities of packages this one needs, for example `["alez/sig"]`. A package whose dependencies are not installed is not loaded.
- Every `.lisp` file under `src/` must begin with a `(module …)` declaration in the package's namespace, or the import is refused.

For example, the manifest of alez/neural is:

```
{
  "name": "alez/neural",
  "version": "0.1.0",
  "entry": "alez.neural.variable-reset"
}
```

**File > Import Package…** accepts the folder directly. Zip it to send it to someone else.

For the process lanes these modules build on, see [Process lanes](process-lanes). For racks that own a graph sequencer, see [Racks](racks). For other tabs beside **Seq**, see [Step sequencer](sequencer-tour).
