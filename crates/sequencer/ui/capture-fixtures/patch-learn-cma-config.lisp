;; Evolutionary Patch Learn configuration with every CMA/Adam stage exposed.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn-cma-config*"
  (box :width :fill :height :fill :background-color :buffer-bg :padding 0.55
    (v-stack :width :fill :height :fill :gap 0.45
      (label "PATCH LEARN — EVOLUTIONARY SEARCH + TRAINING"
        :font-size 11 :color :white :bg :transparent)
      (box :width :fill :height 0 :flex 1
        (scroll :width :fill :height :fill
          (eseq.patch-learn/cma-config)))
      (button "Search, polish, and train winner"
        :variant :primary :width :fill :height 1.45 :color :white))))
