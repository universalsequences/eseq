;; Shared-control palette capture. Select a theme in capture-after-sync;
;; track 0 exercises Space Echo, track 1 Str8 Delay, track 2 instrument sliders.
(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12) :audio-fx ("Space Echo"))
  (track :sampler :name "Snare" :steps (4 12) :audio-fx ("Str8 Delay"))
  (track :instrument "Drums/Digi Hat" :name "Hat"
    :steps (0 2 4 6 8 10 12 14)))

(def-song "theme-controls"
  (at 0 :scene 0)
  (at 16 :scene 0 :patterns ((0 1)))
  :end 32)

(def capture-after-sync ()
  (seq-theme-phosphor))
