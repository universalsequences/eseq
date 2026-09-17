# Akai MIDImix

The factory UI automatically loads `eseq.midi-midimix` from
`content/ui/midi-midimix.lisp`. It matches the input named **MIDI Mix** and the
factory assignments on MIDI channel 1. Enable the input in File > Settings if
it was previously disabled. The host changes require rebuilding and restarting
`metal_seq`; the assignments themselves are editable Lisp.

| Control | Action |
| --- | --- |
| Eight channel faders | Top-level track/group volumes, then visible bus volumes in remaining slots |
| Master fader | Mix/master volume |
| Top two knobs | First two available track sends, excluding group buses |
| Bottom knob | First macro of the strip's Instrument Rack, when present |
| Mute | Toggle the strip's mute |
| Solo + Mute | Toggle the strip's solo |
| Hold Solo | Sequence roll; release to resume the normal sequence |
| Record Arm 1–8 | Always select roll rate, before or during a roll |
| Bank Left / Right | Previous / next scene, using the transport's launch quantization |
| Send All | Hardware snapshot of all fader/knob values |

A group occupies one strip even when expanded. Its member tracks and nested
racks never occupy hardware strips. Group faders and mute/solo buttons affect
the group's backing bus. Group knobs do nothing. Empty drum racks retain their
own strip, as in the mixer.

After the last top-level track/group, unused strips control the visible buses
in their mixer display order. Group buses are excluded because their groups
already have strips. Mix/Main is included at its displayed position if a slot
remains, and the dedicated master fader always controls it as well. Bus strips
also support mute/solo; their knobs do nothing.

All eight Record Arm buttons always select the roll rate, even for empty strips
or bus/group strips. From left to right the rates match
keyboard keys 1–8: **1/4, 1/4t, 1/8, 1/8t, 1/16, 1/16t, 1/32, 1/32t**.
Press a rate button to prepare the next roll, then hold SOLO to trigger it at
that rate. Rate buttons also change the rate during a roll. They never change
track/rack recording arm state, and their releases do nothing. Releasing SOLO
stops the roll and keeps the selected rate for the next hold. This dedicated
MIDI gesture works without enabling the keyboard's Roll mode. SOLO + Mute
retains the hardware's solo action while rolling.

Each input holds its roll independently, including alongside a keyboard hold.
Disconnecting or disabling the input releases its hold. Removing the mapping
while held still permits the physical release to stop the roll. Transport stop
or turning Roll mode off cancels all current holds.

Targets are resolved when each message arrives, including after grouping,
deleting tracks or loading a project. The controls continue to work when the
mixer buffer is hidden. Mixer controls for missing strips and sends do nothing;
the rate buttons remain active. The bottom knob follows its strip's track,
independent of selection or record arming. It does nothing for tracks without
an Instrument Rack or first macro. Macro turns use the same parameter-lock and
recording behavior as the on-screen rack macros and keyboard MIDI mappings.
Faders and knobs set absolute values
immediately, without pickup. Bank buttons stop at the first/last scene and
advance from an already queued scene when quantization is enabled. They do not
change which eight mixer strips the faders control.

The mapping is input-only: it does not synchronize the hardware's button LEDs.
The device must use its factory control assignments. Akai documents the editor
and customizable assignments in its [editor guide](https://support.akaipro.com/en/support/solutions/articles/69000856691-akai-pro-midimix-using-the-editor-for-customisation).

SEND ALL transmits the current values of all 33 faders/knobs, so it applies those
values to the mapped controls, including rack macros. It does not provide a separate button
press/release message in the factory preset, and cannot support a reliable
hold-to-roll gesture. This matches the snapshot behavior described in Akai's
[user guide](https://cdn.inmusicbrands.com/akai/attachments/MIDIMIX/MIDImix-UserGuide-v1.0.pdf)
and the captured MIDI output from the connected device.

## Lisp customization

For a driver that gives the device a different name, put this in `init.lisp`:

```lisp
(eseq.midi-midimix/install "Your exact MIDI input name")
```

The generic MIDI layer exposes `:device-name` and `:device-id` on every connected
input message. Inspect `eseq.midi/last-message` after moving a control. A source
can be restricted by name or endpoint ID:

```lisp
(eseq.midi/midi-map
  (eseq.midi/on-device "MIDI Mix"
    (eseq.midi/on-channel 0 (eseq.midi/cc 19)))
  (lambda (value msg) (seq-set-track-volume 0 value)))
```

`on-device-id` restricts a source to one endpoint, including when multiple
devices share a name. Endpoint IDs persist on macOS; Linux IDs are local to the
current MIDI session. Matching ID-specific mappings take precedence over
name-specific mappings, which take precedence over unscoped mappings. This
keeps existing Komplete Kontrol rack-macro mappings independent of MIDImix CCs.
Reinstalling or reevaluating the same sources replaces them instead of adding
duplicates. `midi-unmap` removes an individual source.

Rust supplies ordered connection identity, generic message dispatch, and a
relative scene-launch command. It also exposes `seq-midi-sequence-roll` (MIDI
note message), `seq-midi-sequence-roll-held?` (port index), and
`seq-set-roll-rate` (zero-based rate index 0–7), with host-owned release cleanup.
Device names, CC/note assignments, mixer
ordering policy and button actions are all defined in Lisp. The mixer and the
controller share `eseq.mixer/render-order`.
