(def shd-rate-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def shd-row-height 1.0)
(def shd-tap-width 3.2)
(def shd-cell-width 5.8)

(def shd-param (name)
  (eseq.effects.custom-effect-ui/midi-fx-ui-param midi-fx-ui-current-fx name))

(def shd-param-value (p)
  (eseq.effects.param-controls/fx-param-value-for midi-fx-ui-current-fx p))

(def shd-set-param (p v)
  (eseq.effects.param-controls/param-set-control-value midi-fx-ui-current-fx p v))

(def shd-param-live-value (p)
  (if (get p :value-field)
    (reactive-get "SEQ" (get p :value-field))
    (reactive-value (shd-param-value p))))

(def shd-clamp (v lo hi)
  (if (< v lo) lo (if (> v hi) hi v)))

(def shd-visible-taps ()
  (shd-clamp (round (shd-param-live-value (shd-param "taps"))) 0 6))

(def shd-num (key p step decimals)
  (number-picker
    :key key
    :value (shd-param-value p)
    :min (eseq.effects.param-controls/param-control-min midi-fx-ui-current-fx p)
    :max (eseq.effects.param-controls/param-control-max midi-fx-ui-current-fx p)
    :step step
    :decimals decimals
    :width shd-cell-width
    :height shd-row-height
    :font-size 9
    :on-change (lambda (v) (shd-set-param p v))))

(def shd-taps-picker ()
  (let ((p (shd-param "taps")))
    (subtree :key (str "custom-midi-fx-ui-" midi-fx-ui-current-name
                       "-slot-" (get midi-fx-ui-current-fx :slot-idx)
                       "-taps-number" (eseq.effects.param-controls/param-control-key-mode midi-fx-ui-current-fx p))
      (number-picker
        :key "shd-taps"
        :value (shd-param-value p)
        :min (eseq.effects.param-controls/param-control-min midi-fx-ui-current-fx p)
        :max (eseq.effects.param-controls/param-control-max midi-fx-ui-current-fx p)
        :base-value (eseq.effects.param-controls/param-base-value-prop midi-fx-ui-current-fx p)
        :base-min (eseq.effects.param-controls/param-base-min-prop midi-fx-ui-current-fx p)
        :base-max (eseq.effects.param-controls/param-base-max-prop midi-fx-ui-current-fx p)
        :step 1
        :decimals 0
        :width 3.5
        :height shd-row-height
        :font-size 9
        :on-change (lambda (v) (shd-set-param p (round v)))))))

(def shd-row (tap-label delay transpose velocity pan)
  (h-stack :gap 0.35 :align :center
    (label tap-label
      :width shd-tap-width :height shd-row-height
      :font-size 9 :h-align :center :color :dim :bg :transparent)
    (shd-num (str "shd-" delay) (shd-param delay) 0.01 2)
    (shd-num (str "shd-" transpose) (shd-param transpose) 1 0)
    (shd-num (str "shd-" velocity) (shd-param velocity) 0.01 2)
    (shd-num (str "shd-" pan) (shd-param pan) 0.01 2)))

(def shd-row-index (n)
  (shd-row (str n)
    (str "delay-" n)
    (str "transpose-" n)
    (str "velocity-" n)
    (str "pan-" n)))

(def shd-header ()
  (h-stack :gap 0.35 :align :center
    (label "tap"
      :width shd-tap-width :height shd-row-height
      :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "delay"
      :width shd-cell-width :height shd-row-height
      :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "trn"
      :width shd-cell-width :height shd-row-height
      :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel x"
      :width shd-cell-width :height shd-row-height
      :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "pan"
      :width shd-cell-width :height shd-row-height
      :font-size 8 :h-align :center :color :dim :bg :transparent)))

(def-midi-fx-ui
  (v-stack :gap 0.18
    (h-stack :gap 0.7 :align :center
      (midi-fx-param "rate" :as :dropdown :items shd-rate-labels)
      (label "taps"
        :width 2.3 :height shd-row-height
        :font-size 9 :h-align :right :color :dim :bg :transparent)
      (shd-taps-picker))
    (shd-header)
    (each (range 1 (+ (shd-visible-taps) 1)) |n| (shd-row-index n))))
