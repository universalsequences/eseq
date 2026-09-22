# alez/neural

Factory neural / graph sequencers, shipped as Lisp modules.

| Module | Tab | What it is |
| --- | --- | --- |
| `alez.neural.variable-reset` | `var rst` | Graph-mode neural sequencer: 1–16 nodes, per-node seed / delay / transpose / velocity rules, live N×N weight matrix, max-poly selection modes including `markov`. |

Attach the package from the Packages tab, or `(import alez.neural.variable-reset)`
from any buffer. `(alez.neural.variable-reset/gvr-init-ring-defaults)` writes a
fresh ring patch into the current pattern.

The sequencer instance is named `variable-reset`; the demo copy under
`content/scripts/sequencers/` keeps its own `neural-variable-reset-demo` name so
the two can coexist in one project.
