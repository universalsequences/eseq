
# Notes

## Custom Sound
- dgen server that lets you write "lisp" expressions to create synths real quick
- compiles the synth and adds it to the graph (cached compilation)
- create a library of synths

- In order to do this need a super specific API for "synths": ("gate", "pitch freq") inputs
- (defun-synth (pitch env) list expressions to create synth)
- Server is a swift wrapper around the dgen library that 1. parses the lisp 

## Jaki?

## Cirklon
- current track is shown with cirklon view instead
- sliders for each parameter 
- each step should have several parameters


[tr 1] [kick df] [pattern 1]
[trn] | dur | vel | spd | aux_a | aux_b
_
_     _     _
_  _  _  _  _
_  _  _  _  _
o  o  o  o  o 
 
transpose is -32 to 32 
when selecting a step typing a number and hitting enter should select it
should show curren steps parameter value

should have easy way to go between tracks 
only show one track at a time with some info of the other tracks

# effects
- comb filter
- phaser
- chorus
- filter

- all p-lockable 
- tracks each have their own fx chain

# Remember
- the goal of this should be to quickly do sampling ---
- 

# Modes 
I want to expand the UI to have multiple regions that are all shown vertically one after each other but "focused" -- controlled by tabs (which hould no longer control param selected)
## cirklon (placed at top where it currently is)
## track/instrument parameters: 
Show as several parameters vertically (up/down selects parameter -- once selected can do the "type value and enter" for boolean enter just toggles)
gate: true/false (when false it plays until it completes) attack/release
swing: 50-75% (default 50) 
## track effects

This is where things get interesting. I want all effect parameters to be p-lockable.
There should be a standard set of effects on each track, with each effect laid out horizontally (arrow left/right changes effect selected) and enclosed in a box with effect name at top and again parameters for the effect vertically laid out with arrow up/down controlling which effect parameter we're in and typing to control the value or enter for toggling booleans

If steps are selected (via the gang system) any parameter edits are p-locked to the step, i.e. they switch smoothly at the step and goes back to the track-wide fx parameter setting when it hits a step that has no p-lock for that parameter. Whenever we toggle a step off it should clear the p-locks. P-locked steps should have some sort of visualization in the step-sequencer showing that it has a p-lock

Parameter layout:
[param name] [value]
[slider visualizing the value] (if its numerical)

### multi-mode filter
  - enabled: true/false
  - mode: dropdown with "lowpass" (default), "hipass", "bandpass"
  - cut: 50-10000
  - res: 0.5-16.0

### stereo delay: 
 - wet (wet is always first)
 - synced: true/false (either syncs to bpm or is raw ms)
 - delay time: based on synced it either shows a synced time like 1/4 (enter should bring up a dropdown that you can arrow up or down and hit enter to lock in the exact setting: 1/32, 1/16, 1/16t, 1/8, 1/8t, 1/8., 1/4, 1/4t, 1/4., 1/2, 1
 - feedback: 0-1
 - dampening: 0-1
 - stereo-width: 1-2 (default 1) multiplies delaytime when computing right-ears delay time

# Timebase
- A track parameter and and configurable to aux_a/b.
Timebase:
- options: (16n (default) 32n 16n 16t 8n 8t 4n 4t 2n 2t 1 
- this controls the speed that playhead moves, along with how it quantizes triggers.
- For example 4n would slowly move step to step once every 1/4th step.  Effectively "slowing down". 
- Timebase also controls what the "unit" of a duration is. For example for timebase=4n duration=1 would last 1/4th note.
- Timebase should also exist as a track parameter that is overrided by any aux_a sets in timebase. 
- AuxA is 0-1 so subdivide these values so they land on the various options since it exists as a "bar slider" ini teh step sequencer. though when set to timebase the value shoulda appear on the bar as youre editing (i.e. the pretty timebase value) and doing up/down should skip to each option range (i.e. 1n for 0 up gets you to 2t (which might be 0.07 or whatever), up again gets you to 2n (might be 0.14 or whatever). 
- Quantization should be sample accurate (for example 8t should be sample accurate triplets)
- Its fine if it gets out of phase with rest of the track (for example 16 steps at 8t will take a different amount of time to complete a bar than 16 steps at 16n).
- to deal with this better, we'll introduce another mechanism to control how many steps we actually trigger. For example, maybe we want it to be 12 steps at 8t so that it ends up completeing all the steps in 1 bar or whatever. this will another parameter in track parameters list called "max step" (default: number of steps in track). You'd be able to then edit it to be 12 or something weird to get polymeter.


# Send FX
final section at bottom

- delay (same delay effect we have per track)
- Reverb (use the reverb~) reference implementation

auxA - should be default set to send but can be later configured in the tracks parameter section 
auxA/B options:
- Send
- Timebase
- gate probablity
- note_accumulator 
- vel_accumualtor

# Accumuatlro
Several different accumulators who's value appends to whatever its routed to (for example note_accumulator adds to whatever the steps transpose value is)
acc = clamp(acc + input × amount + offset)
acc: a stored running value (it persists from step to step)
input: where the “delta” comes from each step (could be a constant, a track value, random, AUX A, AUX B, etc.)
amount: scaling
clamp/wrap: optional limits (min/max or modulo wrap)

## How to reset
Many accumulator setups include reset conditions (e.g., reset on bar, on pattern restart, on a certain event). AUX can be used to:
“pull it back” by flipping sign (drive it back toward center),
or change the clamp range so it “squeezes” or “opens up”.

# FM Digitone
- would it be more intresting to create a really deep synth?
- or have the idea of dynamic synths? cant easily define the synth look though

