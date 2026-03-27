# Lisp MIDI FX

Accumulators are extended from 1:1 step mutators into full event processors: one trigger in, N triggers out, at arbitrary musical times. This is the Lisp equivalent of a MIDI FX chain.

## New primitives

### `acc-suppress`
Drop the original trigger. Without this, the original fires alongside any emitted events.

```lisp
(acc-suppress)
```

### `acc-emit`
Schedule a derived trigger at a musical time offset.

```lisp
(acc-emit offset :vel 0.8 :transpose 7)
```

**Offset** is in step fractions by default — `1` means one full step later, `0.5` means halfway through. The step's own timebase (including any p-locked timebase) determines what "one step" means in real time. The Lisp never touches BPM or sample rates.

To use a different timebase for the offset:

```lisp
(acc-emit :8t 1 :vel 0.8)   ; 1 step in 8th-note-triplet time
(acc-emit :32 2 :vel 0.6)   ; 2 steps in 32nd-note time
```

To snap to a grid boundary:

```lisp
(acc-emit 0 :snap :8n :vel 1.0)         ; next 8th note boundary
(acc-emit 0 :snap-nearest :8n :vel 1.0) ; nearest 8th note boundary
```

Emitted events inherit all parameters from the original trigger unless explicitly overridden. `:vel`, `:transpose`, `:track`, `:pan` are the overridable fields.

### `acc-chord`
Returns the chord note list for the current trigger as a Lisp list.

```lisp
(acc-chord)  ; => (0 7 4 12)
```

## Globals available during accumulator evaluation

| Name | Description |
|---|---|
| `acc-step` | 0-based step index of the triggering step |
| `acc-value` | current accumulator value |

## Internal representation

Offsets are stored as **beats** (quarter notes) in `EmittedEvent`. The scheduler converts to sample time using the snapshot's BPM — Lisp never does this conversion.

```rust
pub struct EmittedEvent {
    pub offset_beats: f32,
    pub resolved: ResolvedStep,
}

pub struct AccumulatorEvalOutput {
    pub resolved: ResolvedStep,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: Vec<ScheduledInstrumentParam>,
    pub emitted: Vec<EmittedEvent>,
}
```

Scheduler: `sample_time = trigger.sample_time + (offset_beats * samples_per_quarter) as u64`

## Examples

### Arpeggiator
```lisp
(def-accumulator "arp"
  (acc-suppress)
  (for-each |i|
    (acc-emit i :note (nth (acc-chord) i) :vel 0.8)
    (range 0 (len (acc-chord)))))
```

Step fraction offsets mean the arp automatically adapts to the step's timebase. P-lock the step to `:8t` — the arp becomes triplets.

### Arp descending with velocity shape
```lisp
(def-accumulator "arp-down"
  (acc-suppress)
  (for-each |i|
    (acc-emit i :note (nth (reverse (acc-chord)) i) :vel (- 1.0 (* i 0.1)))
    (range 0 (len (acc-chord)))))
```

### MIDI delay
```lisp
(def-accumulator "delay"
  (for-each |i|
    (acc-emit i :vel (pow 0.6 i))
    (range 1 4)))
```

No `acc-suppress` — original fires, echoes trail it. Delay taps at fixed step intervals with 0.6x velocity decay each time.

### Note repeat
```lisp
(def-accumulator "note-repeat"
  (acc-suppress)
  (for-each |i|
    (acc-emit (* i 0.25) :vel 1.0)
    (range 0 4)))
```

`0.25` subdivides the step into quarters. Change to `0.125` for eighths. Or use an explicit timebase to anchor the rate independently of the step:

```lisp
(acc-emit :16n i :vel 1.0)
```

### Snap to grid
```lisp
; 4T hit quantized to the next 8th note boundary
(acc-emit :4t 0 :snap :8n :vel 1.0)
(acc-emit :4t 1 :snap :8n :vel 0.8)
```
