;; Capture --buffer fx --track 0/1/2 to inspect cyan, teal, and theme-colored
;; displays with selected-step locks alongside ordinary parameter readouts.
(capture-project
  (track :instrument "factory:Drums/Membrane Snare" :steps (0)
    :instrument-locks ((0 "head_couple" 0.58) (0 "bottom_mix" 2.0)))
  (track :instrument "factory:Physical Models/PM Flute" :steps (0)
    :instrument-locks ((0 "breathnoise" 0.4)))
  (track :instrument "factory:Synths/Heat" :steps (0)
    :instrument-locks ((0 "unison_detune_cents" 25) (0 "glide_time_ms" 150))))
(seq-select-step 0)
