# DGenLisp Modulation Mini Spec

## Purpose

This document specifies a first-pass modulation language feature for DGenLisp.

The goal is to let instrument authors declare modulatable destinations in DSP code
without manually creating source selectors, modulation depth parameters, or
destination-specific resolution code.

This is intended for synth-style workflows similar to Elektron machines:

- modulation source: selected in host/UI
- modulation destination: declared in DSP
- modulation amount: controlled in host/UI

## Design Goals

- Keep authoring simple
- Keep modulation destination handling explicit
- Generate host-visible metadata in the manifest
- Avoid per-engine manual modulation boilerplate
- Support destination-specific modulation behavior

## Non-Goals For V1

- Multiple modulation lanes per destination
- Arbitrary matrix summing in DSP
- Modulation source mixing
- Dynamic creation/destruction of modulation buses
- Host UI spec beyond the data needed from the manifest

## Core Language Additions

### 1. Modulator Inputs

Inputs may be marked as modulation buses.

Example:

```lisp
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
```

Rules:

- `@modulator <slot>` marks an input as a modulation bus
- slot indices are 1-based positive integers
- slot ids should be unique within a patch
- these inputs remain ordinary signals in DSP, but are also exported in the manifest as modulation sources

### 2. Modulation Destination Declaration

Parameters may be declared modulatable with explicit mode metadata.

Example:

```lisp
(param cutoff
  @default 2400
  @min 60
  @max 12000
  @unit Hz
  @mod true
  @mod-mode additive)
```

Additional optional attributes:

```lisp
@mod-depth-min -6000
@mod-depth-max 6000
@mod-unit Hz
```

V1 required attributes:

- `@mod true`
- `@mod-mode <mode>`

V1 supported modes:

- `additive`
- `multiplicative`
- `semitone`

If omitted:

- `@mod` defaults to `false`
- `@mod-mode` is invalid unless `@mod true` is present
- `@mod-depth-min/max` default by mode if omitted

### 3. Modulated Value Access

Modulated values are accessed explicitly with:

```lisp
(mod cutoff)
```

Example:

```lisp
(def filtered (biquad sig (mod cutoff) resonance 1 0))
```

This is preferred over silently rewriting every reference to `cutoff`, because:

- it is explicit at the use site
- it is easier to reason about in compiler implementation
- it avoids ambiguous symbol semantics

## Semantics

For each modulatable parameter, the compiler generates:

- the original base parameter
- one hidden modulation source selector parameter
- one hidden modulation depth parameter
- one internal expression that resolves the final modulated value

For a destination `cutoff`, generated internal symbols are conceptually:

- `cutoff__mod_source`
- `cutoff__mod_depth`
- `cutoff__resolved`

These names are illustrative. Exact internal naming may differ, but must be reserved and collision-safe.

## Generated Parameters

For:

```lisp
(param cutoff
  @default 2400
  @min 60
  @max 12000
  @unit Hz
  @mod true
  @mod-mode additive)
```

the compiler generates hidden host parameters:

```lisp
(param cutoff__mod_source @default 0 @min 0 @max N)
(param cutoff__mod_depth @default 0 @min -6000 @max 6000 @unit Hz)
```

Where:

- `N` is the number of declared modulation buses
- source `0` means `off`
- source `1..N` selects the corresponding `@modulator` input

These generated parameters must appear in the manifest and be linked back to the base destination.

## Selector Semantics

The compiler must resolve:

```lisp
(selector idx mod1 mod2 mod3 mod4)
```

with semantics:

- `0` => `0`
- `1` => `mod1`
- `2` => `mod2`
- `3` => `mod3`
- `4` => `mod4`

Implementation options:

- add a real `selector` primitive to the language/runtime
- or lower it during codegen into nested `gswitch`

Either is acceptable for V1. A dedicated primitive is preferred.

## Codegen Rules

### Additive Mode

Intended for:

- cutoff
- morph
- formant
- comb amount
- drive-like offsets

Generated form conceptually:

```lisp
(def cutoff__mod_selected
  (selector cutoff__mod_source mod1 mod2 mod3 mod4))

(def cutoff__resolved
  (clip (+ cutoff (* cutoff__mod_selected cutoff__mod_depth))
        cutoff_min
        cutoff_max))
```

Where:

- `cutoff_min` is the original param min
- `cutoff_max` is the original param max

### Multiplicative Mode

Intended for:

- rates
- times
- gains when explicitly desired

Generated form conceptually:

```lisp
(def rate__mod_selected
  (selector rate__mod_source mod1 mod2 mod3 mod4))

(def rate__resolved
  (clip (* rate (+ 1 (* rate__mod_selected rate__mod_depth)))
        rate_min
        rate_max))
```

Default depth range for multiplicative mode should be conservative, for example:

- `@mod-depth-min -1`
- `@mod-depth-max 1`

### Semitone Mode

Intended for:

- pitch
- oscillator tune
- FM ratios if treated musically rather than linearly

Generated form conceptually:

```lisp
(def pitch__mod_selected
  (selector pitch__mod_source mod1 mod2 mod3 mod4))

(def pitch__resolved
  (* pitch
     (exp (* (log 2)
             (/ (* pitch__mod_selected pitch__mod_depth) 12)))))
```

Depth in semitone mode is measured in semitones.

Recommended default depth range:

- `@mod-depth-min -24`
- `@mod-depth-max 24`

## `(mod name)` Semantics

The expression:

```lisp
(mod cutoff)
```

means:

- resolve the modulated value for parameter `cutoff`
- if `cutoff` is not declared modulatable, compilation fails with a clear error

Example error:

`mod: parameter 'cutoff' is not declared with @mod true`

## Manifest Additions

The manifest must include modulation bus metadata and modulation destination metadata.

### Modulator Inputs

Example:

```json
"modulators": [
  { "slot": 1, "inputChannel": 4, "name": "mod1" },
  { "slot": 2, "inputChannel": 5, "name": "mod2" },
  { "slot": 3, "inputChannel": 6, "name": "mod3" },
  { "slot": 4, "inputChannel": 7, "name": "mod4" }
]
```

Note:

- `inputChannel` is zero-based if the rest of the manifest is zero-based
- it should match existing manifest conventions

### Modulation Destinations

Example:

```json
"modDestinations": [
  {
    "name": "cutoff",
    "paramCellId": 17,
    "mode": "additive",
    "sourceCellId": 101,
    "depthCellId": 102,
    "min": 60,
    "max": 12000,
    "unit": "Hz"
  }
]
```

Required fields:

- `name`
- `paramCellId`
- `mode`
- `sourceCellId`
- `depthCellId`
- `min`
- `max`

Optional:

- `unit`
- `depthMin`
- `depthMax`

## Host Expectations

The host can render a modulation UI using manifest metadata only.

For each `modDestination`, the host can display:

- destination name
- source selector bound to `sourceCellId`
- modulation amount bound to `depthCellId`

For each `modulator`, the host can display valid available sources.

V1 host model:

- one source selector per destination
- one amount control per destination
- source `0` = off

## Defaults

Recommended default generated depth ranges:

- `additive`
  - default min/max: same unit as destination
  - if unit is Hz and no explicit range is provided, use a conservative range such as `-(max-min)` to `+(max-min)` or require explicit declaration
- `multiplicative`
  - `-1 .. 1`
- `semitone`
  - `-24 .. 24`

Compiler may require explicit `@mod-depth-min/max` for additive mode in V1 if automatic inference is too ambiguous.

## Validation Rules

Compilation should fail when:

- `(mod foo)` references a non-parameter symbol
- `(mod foo)` references a parameter without `@mod true`
- a modulatable parameter declares an unknown `@mod-mode`
- two inputs declare the same `@modulator` slot
- a patch declares zero modulation buses but uses `@mod true`

## Example

Source:

```lisp
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))

(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(param cutoff
  @default 2400
  @min 60
  @max 12000
  @unit Hz
  @mod true
  @mod-mode additive
  @mod-depth-min -6000
  @mod-depth-max 6000)

(param pitch_hz
  @default 440
  @min 20
  @max 20000
  @unit Hz
  @mod true
  @mod-mode semitone)

(def osc (sin (* twopi (phasor (mod pitch_hz)))))
(def filtered (biquad osc (mod cutoff) 0.8 1 0))
(out filtered 1 @name audio)
```

## V1 Recommendation

Implement exactly this scope first:

- `@modulator <slot>` on `in`
- `@mod true` and `@mod-mode <mode>` on `param`
- optional `@mod-depth-min/max`
- `(mod paramName)` expression
- generated hidden source/depth params
- `modulators` in manifest
- `modDestinations` in manifest
- one source + one depth per destination

Do not implement multiple lanes per destination until this simpler model is proven.

## Future Extensions

- multiple modulation lanes per destination
- destination-specific shaping curves
- source polarity options
- smoothing per destination
- host-declared virtual sources beyond physical mod buses
- direct source names instead of only slot indices
