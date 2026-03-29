;; metal-seq-fx.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by metal-seq-grid.lisp.

(defwidget fx-panel-bg
  :width 1 :height 1
  :shader (sdf/layer
    (sdf/fill (+
        (* 0.05 (smoothstep 0 0.1 (* x y)))
        (sdf/rounded-rect (* 1 width) (* 1 height) 0.12))

      (material
        :color
        (mix
          :gray
          (rgba 0.10 0.10 0.11 1)
          (smoothstep 0 0.005 (- (abs d) 0.008))
          ) ))))

(defwidget compile-progress
  :width 12 :height 0.3
  :state (active)
  :shader
  (if (= active 0)
    (rgba 0 0 0 0)
    (let ((bar-w 0.3)
          (pos (fract (* 0.5 itime)))
          (bar-x (- (* pos (+ 1 bar-w)) (/ bar-w 2)))
          (d-bar (- (abs (- x bar-x)) (/ bar-w 2)))
          (bg (sdf/rounded-rect width height 0.06))
          (mask (max bg (- d-bar))))
      (sdf/layer
        (sdf/fill bg
          (material :color (rgba 0.15 0.15 0.17 1)))
        (sdf/fill mask
          (material :color
            (mix
              (rgba 0.3 0.5 1.0 1)
              (rgba 0.2 0.35 0.8 1)
              (smoothstep -0.02 0.02 d-bar))))))))

(effect-buffer "*fx*"
  (v-stack :padding 1 :gap 1
    (h-stack :gap 1
      (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
        (box :background "fx-panel-bg" :padding 1.5
          (v-stack :gap 0.5
            (label (get fx :name) :font-size 12 :color :white :bg :transparent)
            (h-stack :gap 1.5
              (each (chunks (get fx :params) 4) |chunk ci|
                (v-stack :gap 0.5
                  (each chunk |p pi|
                    (h-stack :gap 0.5 :align :center
                      (label (get p :name) :font-size 9 :width 6
                             :color :gray :bg :transparent)
                      (number-picker :value (get p :value)
                        :min (get p :min) :max (get p :max) :decimals 2
                        :on-change (lambda (v)
                          (if (seq-has-selection?)
                            (seq-set-effect-plock (get fx :slot-idx) (get p :idx) v)
                            (seq-set-effect-param (get fx :slot-idx) (get p :idx) v)))
                        :width 8 :height 1.3 :font-size 11)
                      (hslider :width 10 :min (get p :min) :max (get p :max)
                               :value (get p :value)
                               :material (aqua-slider-material)
                               :on-change (lambda (v)
                                 (if (seq-has-selection?)
                                   (seq-set-effect-plock (get fx :slot-idx) (get p :idx) v)
                                   (seq-set-effect-param (get fx :slot-idx) (get p :idx) v))))))))))))
      ;; Add effect
      (box :background "fx-panel-bg" :padding 1.5
        (v-stack :gap 0.5 :align :center
          (label "+" :font-size 12 :color :gray :bg :transparent)
          (dropdown :value ""
            :options SEQ.available-effects
            :placeholder "Add Effect"
            :on-change (lambda (v)
              (host-command "add-effect" (dict :name v)))
            :width 12 :height 1.5 :font-size 11)
          (compile-progress
            :active (if SEQ.compiling 1 0)
            :width 12 :height 0.3))))))
