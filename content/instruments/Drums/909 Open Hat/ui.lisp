;; Performance surface. Fitted scalars remain in the DSP/presets, but are not
;; controls here. These are analytic source envelopes, not an audio waveform.
(def idhat-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idhat-bind (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def idhat-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s 0 name title 7.0 3.6 2.35
    (idhat-c) decimals :linear :widget-knob-track 10 9.5 :center))
(def idhat-panel (title a at ad b bt bd)
  (box :width 14.8 :height 4.8 :padding 0.2 :background-color :instrument-group-bg
    (v-stack :gap 0.15
      (box :width 7 :height 0.7 :background-color (idhat-c)
        (label title :width 7 :height 0.7 :h-align :center :v-align :center
          :font-size 8 :color :black :bg :transparent))
      (h-stack :gap 0.2 (idhat-knob a at ad) (idhat-knob b bt bd)))))

;; One-second horizontal axis, squared to resolve the attack. Each lane is
;; g * metal * ramp * exp(mode_decay*t + d_mode*held_time/decay).
;; All lanes share a 0.4 amplitude ceiling, just above the largest fitted gain.
;; Higher Metal values can clip visually; this is a fixed display scale.
(defwidget eseq-hat-mode-envelope
  :width 30 :height 0.35
  :state (gain rate tail decay metal attack hold)
  :bindable (gain rate tail decay metal attack hold)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 1) 2) 0 1))
        (t (* u u))
        (ramp (clamp (/ (* t 1000) (max attack 0.1)) 0 1))
        (held (max 0 (- t (* hold 0.001))))
        (a (clamp (/ (* gain (clamp metal 0 4) ramp
          (exp (+ (* rate t) (/ (* tail held) (clamp decay 0.1 6))))) 0.4) 0 1)))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :yellow)
      (sdf/paint (sdf/rect aspect 0.012) (rgba 0 0 0 0.15))
      (sdf/paint (- (abs y) (* 0.85 a)) :black))))

;; Actual wash VCA, including its independent fast transient. The envelope
;; excludes the filtered noise and stochastic swish, which have no fixed trace.
(defwidget eseq-hat-wash-envelope
  :width 35 :height 1.65
  :state (tail fast amount decay wash attack hold gain)
  :bindable (tail fast amount decay wash attack hold gain)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 1) 2) 0 1))
        (t (* u u))
        (ramp (clamp (/ (* t 1000) (max attack 0.1)) 0 1))
        (held (max 0 (- t (* hold 0.001))))
        (a (clamp (/ (* gain (clamp wash 0 4) ramp
          (+ (exp (/ (* tail held) (clamp decay 0.1 6))) (* amount (exp (* fast t))))) 4) 0 1)))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :yellow)
      (sdf/paint (sdf/translate 0 0.8 (sdf/rect aspect 0.01)) (rgba 0 0 0 0.3))
      (sdf/paint (max (- y 0.8) (- (- 0.8 (* 1.5 a)) y)) (rgba 0 0 0 0.12))
      (sdf/paint (- (abs (- y (- 0.8 (* 1.5 a)))) 0.025) :black))))

(def idhat-mode (index)
  (let ((prefix (str "m" index))
        (gesture (eseq.effects.drum-surface/parameter-gesture "decay" "metal")))
    (h-stack :width 35 :height 0.35 :gap 0.2
      (label (str (round (* (reactive-value (idhat-bind (str prefix "f")))
          (pow 2 (/ (reactive-value (idhat-bind "tune")) 12)))))
        :width 4.8 :height 0.35 :font-size 6.7 :v-align :center :color :black :bg :transparent)
      (eseq-hat-mode-envelope :debug-name (str "hat-mode-" index)
        :on-mouse-down (get gesture :down) :on-drag (get gesture :drag) :on-mouse-up (get gesture :up)
        :gain (idhat-bind (str prefix "g")) :rate (idhat-bind (str prefix "d"))
        :tail (idhat-bind "d_mode") :decay (idhat-bind "decay") :metal (idhat-bind "metal")
        :attack (idhat-bind "atk") :hold (idhat-bind "hold")))))
(def idhat-display ()
  (box :debug-name "hat-display" :width 35.6 :height 9.8 :padding 0.3 :background-color (idhat-c)
    (v-stack :gap 0.14
      (label "909 OPEN HAT / DECAY" :width 35 :height 0.7 :font-size 8
        :h-align :center :v-align :center :color (idhat-c) :bg :black)
      (label "METAL / Hz" :height 0.5 :v-align :center :font-size 7.5 :color :black :bg :transparent)
      (v-stack :gap 0.05
        (idhat-mode 1) (idhat-mode 2) (idhat-mode 3) (idhat-mode 4)
        (idhat-mode 5) (idhat-mode 6) (idhat-mode 7) (idhat-mode 8)
        (idhat-mode 9) (idhat-mode 10) (idhat-mode 11) (idhat-mode 12))
      (label "WASH" :height 0.5 :v-align :center :font-size 7.5 :color :black :bg :transparent)
      (let ((gesture (eseq.effects.drum-surface/parameter-gesture "decay" "wash")))
      (eseq-hat-wash-envelope :debug-name "hat-wash"
        :on-mouse-down (get gesture :down) :on-drag (get gesture :drag) :on-mouse-up (get gesture :up)
        :tail (idhat-bind "d_tail") :fast (idhat-bind "d_fast") :amount (idhat-bind "a_fast")
        :decay (idhat-bind "decay") :wash (idhat-bind "wash") :gain (idhat-bind "wash_amp")
        :attack (idhat-bind "atk") :hold (idhat-bind "hold"))))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (idhat-panel "PLAY" "tune" "Tune" 1 "decay" "Decay" 2)
      (idhat-panel "SOURCES" "metal" "Metal" 2 "wash" "Wash" 2))
    (idhat-display)
    (v-stack :gap 0.2
      (idhat-panel "COLOR" "bright" "Bright" 2 "swish" "Swish" 2)
      (idhat-panel "OUTPUT" "drive" "Drive" 2 "level" "Level" 2))))
