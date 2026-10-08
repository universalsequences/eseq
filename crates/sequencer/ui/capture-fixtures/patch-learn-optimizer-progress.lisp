;; Live optimizer feedback for the two non-scalar Patch Learn stages.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn-optimizer-progress*"
  (h-stack :width :fill :height :fill :gap 0.75 :padding 0.75
    (box :width 0 :height :fill :flex 1
      (eseq.patch-learn/training-panel
        (dict :target-path "target.wav" :target-name "Evolution target"
              :stage "cma-es" :current-epoch 3 :total-epochs 12 :loss 0.031
              :losses (list 0.081 0.049 0.031)
              :optimization-losses (list 0.031 0.044 0.052 0.067 0.081)
              :epoch-params (list))))
    (box :width 0 :height :fill :flex 1
      (eseq.patch-learn/training-panel
        (dict :target-path "target.wav" :target-name "Evolution target"
              :stage "cma-refine-batched" :current-epoch 4 :total-epochs 8 :loss 0.018
              :losses (list 0.042 0.031 0.024 0.018)
              :optimization-losses (list 0.018)
              :epoch-params (list))))))
