# Sequencer Tour

The `*sequencer*` buffer is a step grid: one row per track, sixteen steps
per bar. Click a step to toggle it, or use the *musical typing* keys to
enter notes from the computer keyboard.

## Parameter locks

A **p-lock** pins a parameter value to one step. Hold a step and turn a
knob, and only that step is affected. P-locks show as a coloured corner on
the step.

## Rolls

A roll retriggers a step several times inside its own length. From Lisp:

```lisp
(seq-roll :track 0 :step 4 :count 3)
```

1. Select a step
2. Press `r` to open the roll editor
3. Choose a count and a curve

See also [Mixer](mixer) for balancing tracks once a pattern is playing.
