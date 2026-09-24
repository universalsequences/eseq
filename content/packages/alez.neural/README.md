# alez/neural

Factory neural / graph sequencers, shipped as Lisp modules.

| Module | Tab | What it is |
| --- | --- | --- |
| `alez.neural.variable-reset` | one per instance | The `neural` kind, a graph-mode neural sequencer: 1–16 nodes, per-node seed / delay / transpose / velocity rules, live N×N weight matrix, max-poly selection modes including `markov`. |

Attach the package from the Packages tab, or `(import alez.neural.variable-reset)`
from any buffer. That registers the `neural` kind (`alez/neural:neural`) and
creates nothing: create instances from the module row ("New neural") or a rack
menu ("New neural in rack"). Each instance publishes its own sequencer
(`neural#<id>`), keeps its own overrides and view state, and gets its own
`*neural · <label>*` tab. A fresh instance starts on a ring patch (the kind's
`:on-create`); `(alez.neural.variable-reset/gvr-init-ring-defaults (instance-ref id))`
writes it again.

The demo copy under `content/scripts/sequencers/` is the legacy kind-less
script (`neural-variable-reset-demo`, one `*variable-reset*` buffer).
