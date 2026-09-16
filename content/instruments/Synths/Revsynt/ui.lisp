;; Revsynt: FM stab into a note-tuned, crossfading per-voice reverb tank.
;; The eight allpass stage lengths (149..911 samples per unit of delay/10),
;; drawn against the full Size range so headroom for modulation is visible.
(defwidget rv-allpass-view
  :width 35.2 :height 3.2 :state (size) :bindable (size)
  :shader
  (let ((lift (* 0.775 (sqrt (clamp (/ size 4) 0.001 1)))))
    (sdf/layer
      (sdf/region :allpass (sdf/rect width height) :yellow)
      (sdf/paint (sdf/translate 0 0.8 (sdf/rect aspect 0.008)) (rgba 0 0 0 0.3))
      (sdf/paint (sdf/translate (* aspect -0.875) (- 0.8 (* 0.164 lift)) (sdf/rect (* aspect 0.07) (* 0.164 lift))) :black)
      (sdf/paint (sdf/translate (* aspect -0.625) (- 0.8 (* 0.190 lift)) (sdf/rect (* aspect 0.07) (* 0.190 lift))) :black)
      (sdf/paint (sdf/translate (* aspect -0.375) (- 0.8 (* 0.344 lift)) (sdf/rect (* aspect 0.07) (* 0.344 lift))) :black)
      (sdf/paint (sdf/translate (* aspect -0.125) (- 0.8 (* 0.381 lift)) (sdf/rect (* aspect 0.07) (* 0.381 lift))) :black)
      (sdf/paint (sdf/translate (* aspect 0.125) (- 0.8 (* 0.627 lift)) (sdf/rect (* aspect 0.07) (* 0.627 lift))) :black)
      (sdf/paint (sdf/translate (* aspect 0.375) (- 0.8 (* 0.666 lift)) (sdf/rect (* aspect 0.07) (* 0.666 lift))) :black)
      (sdf/paint (sdf/translate (* aspect 0.625) (- 0.8 (* 0.974 lift)) (sdf/rect (* aspect 0.07) (* 0.974 lift))) :black)
      (sdf/paint (sdf/translate (* aspect 0.875) (- 0.8 (* 1.000 lift)) (sdf/rect (* aspect 0.07) (* 1.000 lift))) :black))))
(def rv-allpass ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (rv-allpass-view :debug-name "rv-allpass" :size (eseq.effects.mnm-surface/mnm-bind "size")
      :on-mouse-down (lambda (x y region) (set! gesture.start (list x (eseq.effects.mnm-surface/mnm-value "size"))))
      :on-mouse-up (lambda (x y region) (set! gesture.start nil))
      :on-drag (lambda (x y region)
        (if gesture.start
          (eseq.effects.mnm-surface/mnm-write scope "size" (* (nth gesture.start 1) (exp (- x (nth gesture.start 0))))) false)))))
(def rv-stab-page ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.25
      (adsr-editor :mode :decay :debug-name "rv-stab-decay" :width 35.2 :height 3
        :curve-color :black :point-color :black :grid-color :black
        :background-color (eseq.effects.mnm-surface/mnm-accent)
        :initial 1 :initial-min 0 :initial-max 1 :initial-editable false
        :time (eseq.effects.mnm-surface/mnm-bind "stab_decay_ms") :time-max 2000 :decay-db 60
        :on-change (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
            (list (list :time "stab_decay_ms")) env)))
      (eseq.effects.mnm-surface/mnm-caption "Two cosine operators share a slow triangle shaper swept at FM Rate.")
      (eseq.effects.mnm-surface/mnm-caption "Stab decay after a fixed 19 ms attack. Unison detunes operator two.")
      (h-stack :gap 0.3
        (eseq.effects.mnm-surface/mnm-num "fm_rate" "FM Rate Hz" 2)
        (eseq.effects.mnm-surface/mnm-num "unison" "Unison st" 2)
        (eseq.effects.mnm-surface/mnm-num "stab_decay_ms" "Stab Decay ms" 0)
        (eseq.effects.mnm-surface/mnm-num "gain" "Level" 2)))))
(def rv-tank-page ()
  (v-stack :gap 0.3
    (eseq.effects.mnm-surface/mnm-caption "Feedback delay tuned to the note period x 2^Octave, lowpass in the loop.")
    (eseq.effects.mnm-surface/mnm-caption "A note change moves one of two taps, then crossfades over Fade s,")
    (eseq.effects.mnm-surface/mnm-caption "so the ringing tail never pitch-bends. The amp envelope gates the output.")
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "feedback" "Feedback" 2)
      (eseq.effects.mnm-surface/mnm-num "damping" "Damping Hz" 0)
      (eseq.effects.mnm-surface/mnm-num "octave" "Octave" 0)
      (eseq.effects.mnm-surface/mnm-num "fade_s" "Fade s" 2))))
(def rv-diffuse-page ()
  (v-stack :gap 0.25
    (rv-allpass)
    (eseq.effects.mnm-surface/mnm-caption "Eight allpass stages follow the delay length / 10, scaled by Size.")
    (eseq.effects.mnm-surface/mnm-caption "Fine retunes all stages in cents. Modulate either for phaser movement.")
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "size" "Size" 2)
      (eseq.effects.mnm-surface/mnm-num "size_fine" "Fine cents" 1)
      (eseq.effects.mnm-surface/mnm-num "wet" "Wet" 2))))
(def rv-amp-page ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.15
      (adsr-editor :debug-name "rv-amp" :width 35.2 :height 3.2
        :curve-color :black :point-color :black :grid-color :black
        :background-color (eseq.effects.mnm-surface/mnm-accent)
        :attack (eseq.effects.mnm-surface/mnm-bind "amp_attack_ms")
        :decay (eseq.effects.mnm-surface/mnm-bind "amp_decay_ms")
        :sustain (eseq.effects.mnm-surface/mnm-bind "amp_sustain")
        :release (eseq.effects.mnm-surface/mnm-bind "amp_release_ms")
        :attack-max 1000 :decay-max 2000 :release-max 5000
        :on-change (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" env)))
      (eseq.effects.mnm-surface/mnm-caption "Gates the tank output: a retriggered voice can reveal the last tail.")
      (h-stack :gap 0.3
        (eseq.effects.mnm-surface/mnm-num "amp_attack_ms" "Attack ms" 1)
        (eseq.effects.mnm-surface/mnm-num "amp_decay_ms" "Decay ms" 1)
        (eseq.effects.mnm-surface/mnm-num "amp_sustain" "Sustain" 2)
        (eseq.effects.mnm-surface/mnm-num "amp_release_ms" "Release ms" 1)))))
(def rv-tune-page ()
  (v-stack :gap 0.3
    (eseq.effects.mnm-surface/mnm-caption "Octave retunes the tank only: -1 rings an octave below the stab.")
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "octave" "Octave" 0)
      (eseq.effects.mnm-surface/mnm-num "gain" "Level" 2))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 0 "STAB" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 0 "fm_rate" "FM Rate" 6.8 2 :log)
          (eseq.effects.mnm-surface/mnm-knob 0 "unison" "Unison st" 6.8 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "stab_decay_ms" "Decay ms" 6.8 0 :log)))
      (eseq.effects.mnm-surface/mnm-panel 1 "TANK" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 1 "feedback" "Feedback" 6.8 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 1 "damping" "Damping Hz" 6.8 0 :log)
          (eseq.effects.mnm-surface/mnm-knob 1 "wet" "Wet" 6.8 2 :linear))))
    (eseq.effects.mnm-surface/mnm-display "REVSYNT" '("STAB" "TANK" "DIFFUSE" "AMP" "TUNE" "OUT")
      (list rv-stab-page rv-tank-page rv-diffuse-page rv-amp-page rv-tune-page rv-tune-page))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 2 "DIFFUSE" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 2 "size" "Size" 7 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 2 "size_fine" "Fine ct" 7 0 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 3 "AMP" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 3 "amp_attack_ms" "Attack ms" 7 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 3 "amp_release_ms" "Release ms" 7 0 :log))))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 4 "TUNE" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 4 "octave" "Octave" 7 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 4 "fade_s" "Fade s" 7 2 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 5 "OUT" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 5 "gain" "Level" 14.2 2 :linear))))))
