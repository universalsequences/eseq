# Slowdown

A native stereo varispeed insert, available as **Slowdown** in the built-in
picker on tracks, buses and rack slots. Saved effect identity: `builtin:Slowdown`.
It lowers pitch and stretches the beginning of each rolling capture interval;
this is not pitch-preserving time stretching or a frozen sample looper.

## Controls

| Control | Range | Behavior |
| --- | --- | --- |
| speed | 0.25–1 | Playback/input rate; 0.5 is one octave down, 0.25 two octaves down |
| sync | on/off | Select beat-relative or millisecond interval |
| time | 40–4000 ms | Restart interval when sync is off |
| beats | 0.125–4 | Restart interval when sync is on |
| smooth | 1–100 ms | Complementary crossfade at each restart; limited to half the interval |
| tone | 200–20000 Hz | One-pole lowpass on the wet signal |
| mix | 0–100% | Linear dry/wet blend |

All six continuous controls support the standard **four effect-modulator
slots**, project-wide macro mapping, and parameter locks. Open **mods**, choose
an LFO, envelope, random, drift or external input in any source dropdown, select
that slot, and turn a destination knob to set its bipolar depth. Each destination
sums all four sources as offsets in the destination's own units: playback
ratios, ms, beats, Hz or percentage points. The result is clamped to the
destination range. The timing controls still latch their effective values at
restart boundaries.

Enabled and sync are switches, not continuous modulation destinations. The
clock dropdown chooses beat sync or free time; the division dropdown offers
musical presets without rounding away custom beat values. Both time controls
remain visible so their mappings can be edited regardless of clock mode.
Readouts show playback ratios, beats, ms, kHz and percent, switching to depth
units in the mods view. Display scaling does not alter stored parameter units.

Speed, tone and mix slew over approximately 5 ms. Time, beats, sync, tempo and
smooth changes latch at the next restart: editing them never teleports the
currently audible read head.

With sync on, restarts are **locked to the transport's beat grid**, like
Halftime: the host pushes the song's beat phase once per block (the same hidden
input the DJ Mixer uses), and a new capture starts whenever the transport
crosses a multiple of `beats`. Pressing play or seeking restarts the capture at
once and the next restart falls on the grid, so a one-bar break with `beats` at
4 is captured from its downbeat. The host phase wraps every eight beats, so a
custom `beats` value that does not divide eight gets one short cycle at the
wrap. While the transport is stopped (or with sync off) the cycle free-runs
from tempo or milliseconds alone, so keyboard auditioning still works. A
restart on the very first block after play from bar 1 can land one block late.
Supported sync tempo is 20–400 BPM (values outside this range are clamped
internally).
Bypass ramps back to exact dry while continuing to record and advance history.

## DSP and storage

A stereo circular buffer records live audio. Each frame increases the active
head's delay by `1 - speed`; at an interval boundary a new head starts near the
write position while the outgoing head continues through the crossfade.
Stereo channels share the head positions and interpolation weights. A
32-tap normalized Hann-windowed sinc interpolator uses a 256-phase table with
linear interpolation between coefficient rows. The read guard is 17 samples,
so interpolation never asks for future input. This short guard and the moving
wet delay are part of the effect, not latency applied to the dry signal.

A single graph-owned allocation holds parameters, double-precision head
positions, coefficients and stereo history. History capacity scales with the
sample rate, allowing four beats at 20 BPM plus the outgoing fade without
reading overwritten samples. At 48 kHz the stereo rings occupy 8 MiB; at
192 kHz, 32 MiB. Processing allocates no memory and takes no locks. Reset clears
history; same-rate migration preserves it; a sample-rate change preserves
parameters and modulation depths but starts fresh sample-domain history.
Four extra graph inputs carry the shared effect modulator's sample-rate signals;
its source parameters belong to the host-owned modulator node, not the audio
buffer allocation.

## Focused validation

```sh
cargo nextest run -p sequencer --lib -E 'test(effects::slowdown::tests::) or test(=app::effects::tests::slowdown_installs_on_all_hosts_and_receives_tempo_and_macro_values)'
cargo nextest run -p sequencer --bin metal_seq -E 'test(slowdown_controls_are_modulatable_and_have_visible_geometry)'
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/slowdown.lisp \
  --buffer fx --track 0 --width 2400 --height 420 \
  --out /tmp/metal-seq-slowdown.png
```

Run the capture command from the repository root. DSP tests cover rate/pitch,
stereo coherence, interpolation images, wraparound, DC gain, clock latching,
modulation stress, exact settled bypass, block partitioning, reset/migration
and history bounds at 8–384 kHz. Tests also drive each source input through
all destinations and connect a real effect-modulator node to the DSP. The
live-graph test checks installation and tempo delivery on all three hosts,
and macro delivery to every continuous track-effect control. UI tests invoke
all four source-dropdown callbacks, verify their LFO editors, and edit each
slot's speed depth through the actual knob callback. Unit/depth metadata is
checked on track, bus and rack projections.

`ui/capture-fixtures/slowdown-mods.lisp` captures the source-selector view.
