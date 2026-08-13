;; Convolution Reverb built-in FX panel.
;; A drop target showing the current impulse-response name; dropping a sample
;; onto it swaps the IR (the cursor changes on hover, like other sample drops).

(module eseq.effects.builtin.convolution-reverb)

(import eseq.effects.builtin.filter-core :refer (eseq.effects.builtin.filter-core/builtin-fx-param))
(import eseq.effects.builtin.dynamics :refer (percent-knob number-knob))

(def %drop-ir (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path))
          (track (get target :track))
          (slot (get target :slot))
          (bus (get target :bus)))
      (if path
        (host-command "set-convolution-reverb-ir"
          (dict :track track :slot slot :bus bus :path path))
        (status "Drop a sample file, not a folder")))))

(def convolution-reverb-ui (fx)
  (let ((params (get fx :params)))
    (let ((mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
          (gain-p (eseq.effects.builtin.filter-core/builtin-fx-param params "gain"))
          (ir-name (get fx :ir-name)))
      (v-stack :gap 0.4
        ;; Impulse-response slot: drop a sample here to swap the IR.
        (box :width :fill :height 2.1 :padding 0.5 :v-align :center :h-align :center
          :background-color :instrument-control-bg :corner-radius 8
          :drop-types (list "sample")
          :drop-meta (dict :kind "conv-reverb-ir"
                           :track SEQ.current-track
                           :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
                           :slot (get fx :slot-idx))
          :drop-hover-border-color :blue
          :on-drop (lambda (event) (%drop-ir event))
          (v-stack :gap 0.15 :align :center
            (label "IMPULSE RESPONSE" :font-size 7.5 :color :dim :bg :transparent)
            (label (if ir-name ir-name "Drop a sample")
              :font-size 10.5 :color :fg :bg :transparent)))
        ;; Wet mix + output gain.
        (h-stack :gap 0.6 :align :center
          (if mix-p
            (percent-knob fx "mix" mix-p)
            (box :width 0 :height 0))
          (if gain-p
            (number-knob fx "gain" gain-p 2)
            (box :width 0 :height 0)))))))
