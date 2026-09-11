;; Real project state for the manual's step and mixer illustrations.
;; Generate with scripts/capture_manual_images.py.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift"
    :num-steps 16 :steps (0 4 8 12)))

(def capture-after-sync ()
  (eseq.sequencer/collapse-all-tracks))
