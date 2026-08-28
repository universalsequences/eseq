# Script → module migration map (eseq-mods.17.1)

Rev 1 (2026-08-28). Companion to `docs/attachable-packages-spec.md` rev 3 §4.
Records the staging step of retiring the Scripts tab: the 15 scripts that live
projects actually reference are now modules in the manifest-free personal
workspace at `~/.eseq.d/packages/local/`.

## 1. What shipped

Each keeper was converted in place under `~/.eseq.d/packages/local/demos/` with
the recipe below. The workspace is an unprefixed load root, so the file path is
the module name (`demos/graph-variable-reset.lisp` ⇒
`(import demos.graph-variable-reset)`). Names were chosen to **survive
promotion**: shipping one of these as a factory package (eseq-2k9p.8) is a pure
file move, with the import name and every project scratch line unchanged.

Conversion recipe (uniform across all 15):

1. `(module demos.<name>)` header under the file's doc comment, then an
   `(export …)` block — private-by-default per `docs/module-export-spec.md`.
2. The seven-name contract defs (`script-buffer-name`, `script-tab-label`,
   `script-sequencer-name`) are deleted and their literals inlined into the
   file's own `seq-register-script-step-sequencer-tab` call.
3. The `script-init-fn` wrapper is deleted and the real init function exported
   (`band-coupling-matrix` had no separate one, so its body became
   `band-init-defaults`).
4. `eseq.seq-script-picker/seq-register-script-source-tab` calls are deleted —
   that is the Scripts-browser source tab, which goes away with the tab itself
   (eseq-mods.17.3).
5. Doc comments now show `(import …)` as the scratch entrypoint and spell live
   calls with their qualified names.

Internal `def`/`defstate` needed no changes: they are namespaced automatically.

## 2. The map

`~/.eseq.d/packages/local/demos/<file>` ← `content/scripts/<source>`

| Module | Local file | Source |
| --- | --- | --- |
| `demos.graph-variable-reset` | `graph-variable-reset.lisp` | `sequencers/graph-neural-variable-reset-demo.lisp` |
| `demos.graph-neural-8x8` | `graph-neural-8x8.lisp` | `sequencers/graph-neural-8x8-demo.lisp` |
| `demos.graph-neural-8x8-reset` | `graph-neural-8x8-reset.lisp` | `sequencers/graph-neural-8x8-reset-demo.lisp` |
| `demos.graph-neural-16` | `graph-neural-16.lisp` | `sequencers/graph-neural-16-demo.lisp` |
| `demos.graph-neural-16-cycle` | `graph-neural-16-cycle.lisp` | `sequencers/graph-neural-16-cycle-demo.lisp` |
| `demos.graph-neural-group-matrix` | `graph-neural-group-matrix.lisp` | `sequencers/graph-neural-group-matrix-demo.lisp` |
| `demos.graph-markov-8x8` | `graph-markov-8x8.lisp` | `sequencers/graph-markov-8x8-demo.lisp` |
| `demos.band-coupling-matrix` | `band-coupling-matrix.lisp` | `sequencers/band-coupling-matrix-demo.lisp` |
| `demos.process-performance-lanes` | `process-performance-lanes.lisp` | `processes/process-project-performance-lanes-demo.lisp` |
| `demos.process-phase3a-ports` | `process-phase3a-ports.lisp` | `processes/process-phase3a-ports-demo.lisp` |
| `demos.process-phase3b-mappable` | `process-phase3b-mappable.lisp` | `processes/process-phase3b-mappable-demo.lisp` |
| `demos.process-phase7-reads` | `process-phase7-reads.lisp` | `processes/process-phase7-reads-demo.lisp` |
| `demos.process-conductor` | `process-conductor.lisp` | `processes/process-conductor-demo.lisp` |
| `demos.process-multi-accumulator` | `process-multi-accumulator.lisp` | `processes/process-multi-accumulator-demo.lisp` |
| `demos.macro-player` | `macro-player.lisp` | `ui/macro-player-demo.lisp` |

This table is the rewrite table for eseq-mods.17.2: a scratch line
`(load "content/scripts/<source>")` becomes `(import <module>)`. Both legacy
path families need it — `project.rs:2812` already rewrites
`crates/sequencer/scripts/` to `content/scripts/`, so the migration can run
after that normalization.

## 3. The sources stay put until eseq-mods.17.3

`content/scripts/**` is unchanged by this step, deliberately:

- Every existing project still holds a `(load "content/scripts/…")` line until
  17.2 rewrites it. Deleting the sources first would break those projects for
  the length of one bead.
- All 15 are read by name from Rust tests (`lisp_host/tests.rs`,
  `scheduler/tests.rs`, `ui/state_values/tests.rs`). Retargeting those belongs
  with the tab/contract deletion in 17.3, which is where the assertions listed
  in §4 have to change anyway.

## 4. Verification, and what changes when a demo becomes a module

The converted files were verified by temporarily standing them in for their
sources and re-running the 17 tests that evaluate those demos. Every module
evaluated cleanly — sequencers published their graph manifests and widget
trees, processes attached their chains. The failures that remained were all one
thing: **module namespacing qualifies names that these tests assert
unqualified.** Concretely, after conversion

- widget stable keys become `demos.graph-variable-reset/graph-variable-reset-…`
  rather than `graph-variable-reset-…`;
- `def-process` / `def-accumulator` class names become
  `demos.band-coupling-matrix/band-voice` rather than `band-voice`, and that
  name is what a process-chain slot persists;
- live calls typed in *scratch* must spell exported handles qualified, e.g.
  `(demos.process-phase3a-ports/phase3a-port-writer-h :pitch 4)`.

The first and third are cosmetic (and the doc comments now teach the third).
The second is a **migration hazard for 17.2**: a project that already saved a
process chain from one of these demos holds slots keyed by the unqualified
class name, so switching its scratch line from `load` to `import` re-attaches
the chain under a new class name and the saved per-slot settings will not match.
Deciding whether 17.2 requalifies saved `class_name`s or accepts the reset is
part of that bead.
