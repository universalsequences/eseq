# Bus Routing and Gate Sequencer Spec

## Goal

Replace the single hardwired reverb send with first-class audio channels:

- instrument tracks
- bus / return channels
- the Mix channel, which is the master bus

The first implementation should support drum-bus style routing, send-return style effects, and master-chain effects without introducing a full control-track system yet.

## Non-Goals

- A general cross-track modulation sequencer.
- A dedicated "control track" instrument type.
- Per-step sliders for every bus effect parameter.
- Arbitrary audio feedback routing.
- Full Ableton-style dynamic send UI in the first pass.

Control tracks can come later as a separate instrument/channel type. This spec focuses on audio routing and a simple bus step gate.

## Channel Model

All audible entities should be represented as mixer channels.

```text
Track channel:
  instrument -> track fx -> route/send stage

Bus channel:
  summed bus input -> bus gate -> bus fx -> bus output

Mix channel:
  summed final input -> mix fx -> DAC
```

The Mix channel is the master bus. Every normal bus and return bus routes to Mix unless the project later supports explicit sub-bus routing.

## Bus Identity And Dynamic Buses

The system should ship with two default buses, but it must not be designed as if there will only ever be two.

Bus add/delete should be part of the data model from the start, even if the first UI only exposes the default buses. Avoid using the bus array index as the stable project identity. Tracks became hard to delete because too much state was indexed directly by track position; buses should not repeat that mistake.

Recommended model:

```rust
pub struct BusId(u64);

pub struct BusChannel {
    pub id: BusId,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub soloed: bool,
    pub gate_sequence: BusGateSequence,
    pub effect_chain: Vec<EffectSlotState>,
    pub output: BusOutput,
}
```

Track routing and sends should refer to `BusId`, not to bus index:

```rust
pub enum TrackOutput {
    Mix,
    Bus(BusId),
    None,
}

pub struct TrackSend {
    pub destination: BusId,
    pub amount: f32,
}
```

The UI can still render buses in a simple ordered list:

```rust
pub struct BusList {
    pub order: Vec<BusId>,
    pub channels: HashMap<BusId, BusChannel>,
}
```

Deleting a bus should be defined up front:

- Remove the bus channel and its graph nodes.
- Remove sends targeting that bus.
- Any track whose main output targeted the deleted bus should be rerouted to `Mix`.
- If a future bus routes to a deleted bus, reroute it to `Mix`.
- Preserve the Mix channel; Mix is not deletable.

This makes adding, deleting, and reordering buses a metadata operation instead of a destructive index compaction problem.

## Default Channels

New projects should create:

- `Bus A`
- `Bus B`
- `Mix`

`Bus A` and `Bus B` should be normal bus records with stable `BusId`s. They should be empty by default, or optionally initialized from a project template. The current builtin reverb should stop being a special global send and should become an ordinary effect loaded into a bus slot if desired.

## Track Routing

Each instrument track gets:

```text
output: Mix | Bus(id) | None
sends: zero or more post-fader sends to buses
```

Meaning:

- `output = Mix`: normal direct-to-master routing.
- `output = Bus A`: group routing, useful for drum-bus compression, saturation, filtering, etc.
- `output = Bus B`: same, with a second default bus.
- `output = None`: sends-only routing.
- sends: parallel post-fader send levels into one or more buses.

Send destinations always return to Mix through their bus channel. They do not bypass Mix.

Track sends should be post-fader only in v1. Pre-fader sends can be added later if needed, but the first version should keep the send model simple and predictable.

This allows both common workflows:

```text
Drum bus:
  Kick/Snare/Hats output -> Bus A -> Mix

Dub delay return:
  Snare output -> Mix
  Snare send -> Bus B delay -> Mix

Sends only:
  Track output -> None
  Track send -> Bus B reverb/delay -> Mix
```

## Mixer UI

The mixer side panel should show buses and Mix as additional rows after tracks.

```text
1  Kick
2  Snare
3  Bass
A  Bus A
B  Bus B
M  Mix
```

Every mixer row should support:

- select
- volume
- mute
- solo
- meter

Instrument track rows keep record-arm and delete controls. Bus rows should not show record-arm. Mix should show neither record-arm nor delete, because Mix is not deletable.

Selecting an instrument track shows the current track sequencer and its instrument/effect panels.

Selecting a bus or Mix selects that channel and shows its effect chain. Mix has no sends and no output selector.

If the UI allows adding/deleting buses in v1, add/delete controls should operate on bus IDs and should not mutate track or bus identity by shifting indices. If the UI does not expose this yet, the underlying project schema should still be ready for it.

## Bus Sequencer

When a bus channel is selected, the main sequencer area should not pretend the bus has an instrument. Instead, it should show a simplified bus gate sequencer.

The bus gate sequencer has:

- step on/off state
- per-step gate amount, using the existing velocity lane/value
- its own pattern length, timebase, swing, and swing resolution controls
- generalized p-lock support for bus effect parameters

Default state:

```text
16 steps on
velocity/gate amount = 1.0
```

This means the bus is continuously open and behaves like a normal bus until edited.

Bus timing is independent from track timing. A bus can have its own timebase and can p-lock that timebase using its own sequencer, just like tracks can. This is a core musical feature, not a later polish item.

Example patterns:

```text
All on:
  normal bus processing

Every other step on:
  rhythmic bus chopping

Steps 4, 5, 6, 11, 12, 13 on:
  rhythmic dub-send accent when used with delay/reverb on the bus
```

## Gate Meaning

The bus step state controls whether signal is allowed through the bus gate during that step.

The bus step velocity controls gate gain:

```text
off step: bus gate gain = 0.0
on step:  bus gate gain = step velocity
```

The bus gate should support smoothing to avoid clicks. The first implementation can use a short fixed ramp; later versions can expose attack/release or shape.

## Gate Continuity And Smoothing

Adjacent full-open steps must behave like one continuous open region.

This is required for the default bus behavior:

```text
16 steps on
duration = 1.0
velocity/gate amount = 1.0
```

That pattern should sound identical to an always-open bus. It should not retrigger the gate envelope every step, and it should not add subtle clicks, pumping, or a repeated "gated" texture.

The gate engine should look ahead across step boundaries and avoid unnecessary ramps when the target gain is unchanged:

```text
step N target gain     = 1.0
step N duration        = 1.0
step N+1 target gain   = 1.0

result:
  hold gain at 1.0 across the boundary
```

Ramps should only occur when the effective gate gain changes, or when a step duration creates an intentional close/reopen shape.

Examples:

```text
1.0 -> 1.0:
  no ramp, no retrigger, continuous open

1.0 -> 0.5:
  smooth ramp down to 0.5

0.0 -> 1.0:
  smooth ramp open

on step with duration < 1.0 followed by another on step:
  close according to duration, then reopen smoothly if that is musically intended
```

Implementation detail: the bus gate should be represented as a continuous gain curve over time, not as independent per-step envelopes that blindly restart at each step boundary.

## Gate Tap Point

Bus gates should eventually support two tap points:

```text
pre-fx:
  bus input -> gate -> bus fx -> Mix

post-fx:
  bus input -> bus fx -> gate -> Mix
```

Recommended first implementation: `pre-fx`.

Reason: pre-fx gating makes send-return workflows musical. For example, if Bus B has a delay and only steps 4-6 are open, those steps feed the delay while the delay tail continues to ring into Mix. This matches the "accent snares into a dub delay" use case.

Post-fx gating is also useful, but it is a different sound: it chops the processed output and cuts delay/reverb tails. It should be added as a bus option after pre-fx is working.

## P-Locks

Bus sequencer steps should use the existing generalized p-lock model.

There should not be special fixed lanes like:

```text
fx slot 1 param
fx slot 2 param
```

Instead, any bus step can carry zero or more p-locks targeting arbitrary parameters in the selected bus channel's effect chain.

Examples:

- step 4 opens the bus gate and p-locks delay feedback higher
- step 8 p-locks filter cutoff on Bus A
- step 16 p-locks Mix limiter drive for a transition

The visible simplified sequencer only needs to expose gate state and gate amount. Detailed p-lock editing can use the same parameter selection/editing workflow used for track effects.

## Mix Channel

Mix is the master bus and should support:

- volume
- mute
- meter
- effect slots
- a gate sequencer and p-lockable effect parameters, using the same structure as other buses

Mix gate sequencing may be musically destructive or confusing, but keeping Mix structurally identical to other buses is likely simpler and safer for implementation. The UI can make Mix gating visually subdued or default it to all-on, but the underlying channel model should not be special-cased more than necessary.

Mix is not deletable and no channel should route around it.

## Solo Semantics

Bus solo should include tracks routed into that bus automatically.

If `Bus A` is soloed:

- tracks with `output = Bus A` should be audible through Bus A
- tracks with post-fader sends into `Bus A` should feed Bus A
- Bus A should route to Mix so the soloed bus can be heard
- unrelated tracks and buses should be muted unless they are needed to hear the solo path

This makes bus solo behave like a musical group solo rather than a silent isolated return.

## Relationship To Future Control Tracks

The bus gate sequencer is intentionally narrow. It solves immediate musical workflows:

- group processing
- return effects
- rhythmic bus chopping
- dub-style send accents
- master/bus effect p-locks

It should not absorb the larger "control track" idea.

Future control tracks can be introduced separately as non-audio channels that emit modulation/control events:

```text
control track -> external mod source on tracks/buses/effects
```

That later system can target instruments, effect parameters, bus gates, and macros. The bus sequencer should remain a simple audio-channel gate plus p-lock surface.

## Open Questions

- Should the first UI expose add/delete bus controls immediately, or should it only make the project/graph model ready?
- Should bus sends allow duplicate destinations, or should each destination appear at most once per track?
- Should deleting a bus remove all p-locks targeting that bus, or should project undo/history preserve enough metadata to restore them?

## Recommended V1

1. Add `BusId`-based bus channels with default `Bus A`, `Bus B`, and non-deletable `Mix`.
2. Replace the global reverb send with ordinary post-fader sends into buses.
3. Route all bus outputs to Mix.
4. Add per-track `output` and a list of post-fader bus sends.
5. Show buses and Mix as mixer rows.
6. Selecting a bus shows bus FX and a gate sequencer with its own timebase.
7. Give Mix the same underlying sequencer structure, defaulted all-on.
8. Implement pre-fx bus gating first.
9. Reuse generalized p-locks for arbitrary bus effect parameters.
10. Make bus solo include tracks routed or sent into that bus.
