# Process lanes

Beside velocity, duration, and the other step parameters, every track carries eight process lanes. A lane is a value per step, like any other parameter lane, but instead of setting a note property directly it feeds a small process that runs just before the step plays. Processes remember things between steps, so a few sparse values become a running transform: a transpose that climbs, a random number that lands on a filter, a hit that repeats more each bar.

The lanes are the same on every track, and each track keeps its own values and its own running state. If you know the Cirklon's accumulators and aux lanes, this is that idea with named lanes.

## The lanes

- **prob** — the chance, 0 to 1, that the step plays at all. A rejected step is silent but the lanes below it still advance.
- **reset** — a step set high clears the three accumulators before that step plays.
- **rand** — draws a new random number between its lo and hi on every step whose lane is high, and holds it otherwise. On its own it does nothing; map its output somewhere.
- **count** — adds the lane value to a counter each step and wraps from hi back to lo. A generator like rand, but predictable.
- **tacc** — accumulates the lane value into a running transpose, added to the step's own transpose. Set 1 on the first step for a line that climbs one semitone per bar.
- **acc A** and **acc B** — two more accumulators with a mappable output. acc A starts on retrig and acc B on rate, so a ramp on either turns into a growing roll.
- **grab** — adds another track's transpose, scaled by the lane, optionally from a few steps ago. Set the source track in the strip.

## Opening a lane

Expand a track, then pick a lane from the dropdown at the right end of the parameter tabs. The grid draws the lane in amber so it never reads as a note property. Sliders, the number picker, typing digits, and selections work exactly as they do for velocity. See [Step sequencer](sequencer-tour).

## The lane strip

With a lane open, a strip appears to the right of the grid. It is the lane's control panel.

- The header shows the lane name. The arrows move the lane earlier or later in the chain, which matters when lanes feed each other.
- **IN** shows where the lane's input comes from: its own values, or another lane wired into it.
- **accumulate** and **pass** appear on the accumulators. Accumulate folds each input into the running value. Pass forwards each input as it arrives, which turns an accumulator into a plain relay for whatever is wired into it.
- **OUT** shows the target the output lands on, and **map** changes it.
- **NOW** shows the running value, with a small scope of its recent history on this track. It stays hidden until the lane has fired.
- **lo** and **hi** set the range the value wraps inside. They also set the range of the lane's sliders.
- grab's **source** is a track picker; **lag** reads that many steps back.

## Mapping an output

Press **map**. Everything the output can land on lights up: the step parameter tabs, the other lanes, and every mappable knob on the track's instrument and effects. Click one to bind it. Click **map** again to cancel.

Mapping onto an output that is already bound adds a second target instead of replacing the first. Each extra target gets its own **lo … hi** range in the strip: the output is rescaled from the lane's lo and hi into that range before it is set. This is how one rand drives velocity between 0.1 and 1 and retrig between 4 and 8 at once. The **×** removes a target.

Step parameters take the value in their own units: 3 on tpose is three semitones, 3 on rtrg is three repeats. A synth or effect knob has no natural unit, so give it a range.

## Wiring lanes together

While a map is armed, the **OTHER LANES** row lists the lanes that can take the output as their input. Click one to wire it. rand into acc A with acc A on pass and mapped to a filter is the classic patch: a new random filter position on every step you choose.

Order counts. Lanes run top to bottom in the dropdown's order, so a lane that feeds one above it lands one step late. The IN chip draws dimmed when that is the case. Use the header arrows to move the writer above the reader.

## This track or every track

Lane values are always per track. Everything else in the strip, the targets, the mode, lo and hi, applies to the current track only, so mapping rand to prob on track 1 leaves the other tracks alone.

To change every track at once, switch the header's **this track** to **all tracks** before editing. A track you have edited individually keeps its own settings; clearing a target on that track returns it to the shared one.

Try it: put 1 on tacc's first step, set reset high on step 1, and play. The pattern climbs a semitone per bar and snaps back when you reset. Then open rand, map it to prob, and hear the pattern thin out at random.
