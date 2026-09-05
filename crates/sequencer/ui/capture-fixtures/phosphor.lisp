;; One phosphor hue across differently colored authored tracks.
;; Capture with --buffer sequencer or --buffer fx.
(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12) :audio-fx ("filter"))
  (track :sampler :name "Snare" :steps (4 12))
  (track :sampler :name "Hats" :steps (0 2 4 6 8 10 12 14)))

(def capture-after-sync ()
  (seq-theme-phosphor))
