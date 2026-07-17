(capture-project
  (track :sampler :name "Sampler"))

(effect-buffer "*queued-scene-preview*"
  (box :background "transport-btn-bg" :padding 0.4 :height 2.0
    (h-stack :gap 0.2 :align :center
      (box :width 2.8 :height 1.2
        :background "pattern-pill-bg"
        (v-stack :align :center
          (label "1" :font-size 11 :color :gray :bg :transparent)))
      (box :width 2.8 :height 1.2
        :background "queued-scene-pill-bg"
        (v-stack :align :center
          (label "2" :font-size 11 :color :white :bg :transparent)))
      (box :width 2.8 :height 1.2
        :background "pattern-pill-bg"
        (v-stack :align :center
          (label "3" :font-size 11 :color :gray :bg :transparent))))))
