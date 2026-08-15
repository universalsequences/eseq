;; Live optimizer feedback for the two non-scalar Patch Learn stages.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn-optimizer-progress*"
  (h-stack :width :fill :height :fill :gap 0.75 :padding 0.75
    (box :width 0 :height :fill :flex 1
      (eseq.patch-learn/training-panel
        "target.wav" "Evolution target"
        "cma-es" 3 12 0.031
        (list 0.081 0.049 0.031)
        (list 0.031 0.044 0.052 0.067 0.081)
        (list)))
    (box :width 0 :height :fill :flex 1
      (eseq.patch-learn/training-panel
        "target.wav" "Evolution target"
        "cma-refine-batched" 4 8 0.018
        (list 0.042 0.031 0.024 0.018)
        (list 0.018)
        (list)))))
