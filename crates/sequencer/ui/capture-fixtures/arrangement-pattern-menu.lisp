(capture-project
  (scenes 3)
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12)))

(def-song "pattern-menu"
  (at 0 :scene 0)
  :end 16)

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (eseq.arrangement/open-placement-menu 0 (dict :sx -0.9 :col 34 :row 7))))

;; Click the parent row through the normal editor pointer route before capture.
(def capture-click-widgets (list "menu-item"))
