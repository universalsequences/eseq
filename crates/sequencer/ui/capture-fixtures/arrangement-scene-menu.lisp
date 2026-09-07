(capture-project
  (scenes 3)
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12)))

(def-song "Scene state" (at 0 :scene 0) (at 8 :scene 1) :end 16)

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (eseq.arrangement/open-scene-menu (dict :sx -0.9 :col 34 :row 5))))

(def capture-click-widgets (list "menu-item"))
