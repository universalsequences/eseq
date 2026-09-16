;; Ten-track project with a long, real instrument/effect strip. Used by the
;; production tiled scroll replay; no audio device is opened.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Synth"
    :audio-fx ("EQ8" "Space Echo" "Str8 Delay" "Phaser-Flanger"))
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12))
  (track :sampler :name "Hat" :steps (2 6 10 14))
  (track :sampler :name "Percussion")
  (track :sampler :name "Bass")
  (track :sampler :name "Keys")
  (track :sampler :name "Pad")
  (track :sampler :name "Texture")
  (track :sampler :name "Return"))
