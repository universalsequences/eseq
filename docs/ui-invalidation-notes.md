# UI Invalidation Notes

Goal: keep large declarative Lisp UIs fast without requiring hand-tuned buffer structure.

## Core idea

The runtime should treat the widget tree as a stable, incrementally reusable graph, not as something that must be rebuilt and fully relaid out every time any reactive input changes.

## What the system needs

1. Stable subtree identity.
Each emitted widget/subtree should have a stable identity across reactive runs so the runtime can match "the same thing with new props" instead of treating it as a fresh node.

2. Structural snapshots.
Widget trees stored for buffers should be deep, immutable snapshots. Reuse logic should never observe later mutations through shared cells.

3. Fine-grained reactive ownership.
Instead of one large effect per buffer, the runtime should be able to know which subtree depends on which reactive inputs, so a slider change only reruns the affected subtree.

4. Subtree-level layout reuse.
If size-affecting props are unchanged, the runtime should reuse prior layout geometry for that subtree and only mark render props dirty.

5. Container-specific reuse rules.
Some widgets, like `tabs`, only lay out the selected body. Reuse must compare against the effective rendered children, not the full declarative child list.

6. Incremental render scene updates.
After layout reuse succeeds, the renderer should patch only dirty widget primitives/instance data instead of rebuilding the full scene for the whole buffer.

## Likely runtime model

- Reactive evaluation produces subtree snapshots with stable IDs.
- Dirty reactive inputs mark a bounded set of subtree roots dirty.
- Dirty subtree roots are reevaluated.
- Unchanged subtrees are reused by equality/identity checks.
- Changed subtrees go through subtree relayout only.
- Renderer patches cached primitives for the changed widget IDs.

## Why this is better than Lisp-side tuning

- Buffer scripts can stay simple and declarative.
- Performance does not depend as heavily on manually splitting effects/buffers.
- Complex UIs become fast by default if most edits only affect a small part of the tree.
- The same engine behavior helps every UI, not just `metal_seq`.

## Practical next steps

1. Add stable subtree keys/identity through widget emission.
2. Keep improving size-affecting prop rules per widget type.
3. Track reactive dependencies at subtree granularity, not just whole-buffer effect granularity.
4. Use `dirty_widget_ids` to patch cached Metal scene data incrementally.
5. Preserve compact profiling so hot effects, relayout misses, and full-scene rebuilds remain visible.
