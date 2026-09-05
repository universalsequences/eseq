;; Quantizer panel: the grid division notes snap to.
(def quantizer-grid-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def quantizer-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium "GRID" (eseq.effects.custom-ui-lego/ui-accent-blue)
    (v-stack :gap 0.34 :width 9.0 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "grid" "division" 9.0 quantizer-grid-labels (eseq.effects.custom-ui-lego/ui-accent-blue))
      (label "note starts snap to the nearest line" :font-size 8.2 :width 12.0 :color :dim :bg :transparent))))

(def-midi-fx-ui
  (h-stack :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (quantizer-block))))
