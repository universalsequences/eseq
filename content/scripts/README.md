# Loadable scripts

The Metal sequencer exposes this directory in its Scripts browser. Scripts are
grouped by purpose while retaining the `scripts/` root used by project scratch
loads:

- `processes/` contains process and process-channel demonstrations.
- `sequencers/` contains graph, neural, and conventional sequencer examples.
- `ui/` contains scripts that demonstrate source tabs and UI-facing features.

Python maintenance utilities belong in `../tools/`, not in the loadable script
tree.

## Being retired

This tree is on its way out (eseq-mods.17): a script is just a module on the
load path, and attaching one to a project is an `(import …)` line in *scratch*.
The 15 scripts that live projects reference have already been converted into
modules under `~/.eseq.d/packages/local/demos/` — see
`../../docs/script-module-migration-map.md` for the name map. The sources here
stay until existing project scratches are rewritten (eseq-mods.17.2) and the
Scripts tab, its picker load flow, and the seven-name contract are deleted
(eseq-mods.17.3). Add new work as a module, not as a script here.
