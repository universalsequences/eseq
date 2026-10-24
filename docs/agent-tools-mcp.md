# Agent tools over MCP

`metal_seq mcp` serves eseq's agent tools over the Model Context Protocol
(JSON-RPC 2.0 on stdio). Any MCP client can use them: Claude Code, Codex, or
the in-app agent that will call the same registry (epic eseq-maxc). The first
goal is authoring custom sequencer packages without reading Rust.

## Run it

```sh
cargo build -p sequencer --bin metal_seq
claude mcp add eseq -- "$PWD/target/debug/metal_seq" mcp
```

The server boots one headless app (the `metal_seq capture` boot: engine,
four empty sampler tracks Kick/Snare/Hat/Perc, UI runtime, editor) and keeps
it for the session, so definitions, instances and buffers persist between
calls. `--project SAVED_PROJECT_JSON` starts from a saved project instead.
Boot takes about 9 s in a debug build. macOS only: rendering uses the Metal
capture path.

To test without a client, script the calls:

```sh
ESEQ_CONFIG_DIR=/tmp/eseq-mcp-cfg python3 crates/sequencer/tools/mcp_session.py \
  target/debug/metal_seq calls.json /tmp/eseq-mcp-out
```

## Tools

| Tool | What it does |
|---|---|
| `guide` | An authoring guide: `sequencer`, `eseqlisp`, `instrument`, `effect` |
| `apropos` | Names containing every query word, with signature and first doc line |
| `describe` | One name's signature and docs; a kind's fields |
| `list_modules` | Module names on the load path |
| `search_source` | Grep over every loaded module: matching lines with module, line and the definition around them |
| `outline` | A module's top-level forms: lines, head, name, first doc line |
| `form_source` | The exact text of the forms defining a name |
| `create_module` | A new Local module (`a/b.lisp` under Local packages), imported |
| `define_form` | Replace or append one top-level form, write the file, reload |
| `write_module` | Replace a whole module file and reload it (same restore-on-failure rule) |
| `eval` | Evaluate forms; for questions and experiments, nothing is saved |
| `new_instance` | Create an instance of a kind; returns its buffer |
| `simulate` | Run an instance's tick for N bars headlessly; list its hits or the tick that threw |
| `capture` | Render a buffer (or a keyed subtree) to PNG, with a layout report; `click` a keyed widget and type `keys` first to operate the panel |
| `dgen_docs` | DGenLisp operators, special forms, @attributes and example patches from the compiler's API catalog (`apropos`/`describe` only know eseqlisp) |
| `dgen_values` | What DGenLisp defs in a dsp.lisp come to at given param values: constant arithmetic, `gswitch`, `iota` and math, vectors element-wise; defs reading a signal are reported as varying |
| `view_state` | The reactive state a drawn buffer reads: kind fields grouped across instances (`track.in-selection across 4 track instances: false, except track[1] true`) and other fields, with values and docs |

Rules the tools hold:

- **The file is the truth.** Definitions persist only through
  `define_form` / `create_module`, which write the file and reload it from
  disk. A failed reload restores the file and returns the errors, so live
  code never drifts from source. `eval` is scratch.
- **Read by form, not by line.** `outline` then `form_source`; `define_form`
  matches the existing form by the name it defines (a name defined twice is
  an error, not a guess). Forms with no name (`bind-key`) always append.
- **Only user tiers are writable**: Local modules and installed packages.
- **Every call answers.** Failures come back as `isError` results carrying
  the Lisp errors printed while the call ran (stdout belongs to the
  protocol; everything the app prints is captured per call).
- **Check what a tick plays.** `simulate` runs the instance's tick through
  the scheduler's generator runtime against a copy of its document in the
  selected scene, from a fresh RNG and clock as on Play, merges and routes
  the hits as the scheduler does (accents, rack pads), and lists them by bar
  and sixteenth. It runs on a throwaway state, so nothing a tick writes
  reaches the session; reads of other tracks' patterns see an empty project.
  Downstream processes, groove/swing, `chan-get` values and track harmony
  are not modelled.
- **Views that throw are reported.** A view or effect that throws while a
  call runs is appended to that call's result ("Render errors"), since the
  editor otherwise keeps the last good tree and says nothing.
- **Look at UI as data too.** `capture` lists every keyed widget's pixel
  rectangle with `clipped`, `offscreen` and `zero-size` problems, and
  overlapping siblings in stacks and grids.

## Adding a tool in Lisp

Host tools live in `crates/sequencer/src/ui/mcp/tools.rs`. Anything that can
be written in Lisp goes in the registry, `content/ui/agent-tools.lisp`
(`eseq.agent-tools`), from any module:

```lisp
(import eseq.agent-tools :refer (define-agent-tool))
(define-agent-tool "ring_lengths"
  :doc "Each ring's step count."
  :params '((instance :integer "Instance id") (verbose :bool "Long form" :optional true))
  :run (lambda (args) …))
```

`run` reads `args.instance`. It returns a string (shown as is) or any value
(printed). Host tools win a name clash.
