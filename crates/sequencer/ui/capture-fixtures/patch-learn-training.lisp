;; Patch Learn training state with a multi-order-of-magnitude loss trajectory.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn-training*"
  (box :width :fill :height :fill :background-color :buffer-bg :padding 0.55
    (eseq.patch-learn/training-panel
      "samples/drums/kick.wav"
      "Studio Kick"
      "training"
      7
      50
      15.63
      (list 16.0 15.95 15.89 15.82 15.76 15.69 15.63)
      (list
        (dict :name "cutoff" :from 520 :value 810.4 :change 290.4 :step 0.42)
        (dict :name "resonance" :from 0.2 :value 0.31 :change 0.11 :step 0.18)
        (dict :name "env_mod" :from 3100 :value 2440 :change -660 :step -0.31)))))
