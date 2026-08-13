;; Extreme arrangement zoom fixture: the 128-bar span verifies that ruler
;; labels and vertical grid lines remain readable over dense clip content.

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12))
  (track :sampler :name "Hat" :steps (0 2 4 6 8 10 12 14))
  (track :sampler :name "Perc" :steps (3 7 11 15))
  (track :sampler :name "Bass" :steps ((0 0) (4 -5) (8 -2) (12 -7))))

(def-song "zoomed-out-grid"
  (at 0 :scene 0)
  :end 512)

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (set! eseq.arrangement/view-start 0)
    (set! eseq.arrangement/view-duration 512)
    (eseq.arrangement/set-cursor 128 0)))
