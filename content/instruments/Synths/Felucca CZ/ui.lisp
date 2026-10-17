;; Felucca CZ: the PHASE engine of the Felucca FM-1 firmware.
;; Two cycles of the oscillator at the current DCW, before the envelope:
;; WAVE, then WAVE2 (or WAVE again when WAVE2 is off), as the voice
;; alternates them. Same break points as cz-wave in dsp.lisp.
(defwidget fcz-wave-view
  :width 35.2 :height 4.2 :state (wave wave2 dcw)
  :shader
  (let ((t (/ (+ (/ x aspect) 1) 2))
        (w (if (< t 0.5) (floor wave) (if (> wave2 0.5) (- (floor wave2) 1) (floor wave))))
        (ph (fract (* 2 t)))
        (d (* (clamp dcw 0 1) 0.8545))
        (saw-x (+ 0.5 (* 0.5 d)))
        (sqr-x (max (- 0.5 (* 0.47485 d)) 0.000001))
        (pls-x (max (- 1 (* 0.96874 d)) 0.000001))
        (dsn-x (- 1 (* 0.75 d)))
        (spl-x (- 1 (* 0.5 d)))
        (spl-peak (max (* spl-x (+ 0.5 (* 0.45 d))) 0.000001))
        (bent
          (if (< w 0.5)
            (if (< ph saw-x) (/ (* 0.5 ph) saw-x) (+ 0.5 (/ (* 0.5 (- ph saw-x)) (max (- 1 saw-x) 0.000001))))
            (if (< w 1.5)
              (if (< ph sqr-x) (/ (* 0.5 ph) sqr-x)
                (if (< ph 0.5) 0.5 (if (< ph (+ 0.5 sqr-x)) (+ 0.5 (/ (* 0.5 (- ph 0.5)) sqr-x)) 1)))
              (if (< w 2.5)
                (if (< ph pls-x) (/ ph pls-x) 1)
                (if (< w 3.5)
                  (if (< ph dsn-x) (/ ph dsn-x) (/ (- ph dsn-x) (max (- 1 dsn-x) 0.000001)))
                  (if (< w 4.5)
                    (if (>= ph spl-x) 1
                      (if (< ph spl-peak) (/ (* 0.5 ph) spl-peak)
                        (+ 0.5 (/ (* 0.5 (- ph spl-peak)) (max (- spl-x spl-peak) 0.000001)))))
                    (fract (* ph (+ 1 (* 4.69973 d))))))))))
        (level
          (if (< w 4.5) 1
            (* 0.625
              (if (< w 5.5) (- 1 ph)
                (if (< w 6.5) (if (< ph 0.5) (* 2 ph) (* 2 (- 1 ph)))
                  (min 1 (min (* 4 ph) (* 4 (- 1 ph)))))))))
        (a (* -1 level (cos (* 6.2831853 bent)))))
    (sdf/layer
      (sdf/region :wave (sdf/rect width height) :yellow)
      (sdf/paint (sdf/rect aspect 0.007) (rgba 0 0 0 0.2))
      (sdf/paint (sdf/rect 0.006 height) (rgba 0 0 0 0.2))
      (sdf/paint (- (abs (+ y (* 0.7 a))) 0.022) :black))))
(def fcz-wave ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (fcz-wave-view :debug-name "fcz-wave"
      :wave (eseq.effects.mnm-surface/mnm-bind "wave")
      :wave2 (eseq.effects.mnm-surface/mnm-bind "wave2")
      :dcw (eseq.effects.mnm-surface/mnm-bind "dcw")
      :on-mouse-down (lambda (x y region) (set! gesture.start (list y (eseq.effects.mnm-surface/mnm-value "dcw"))))
      :on-mouse-up (lambda (x y region) (set! gesture.start nil))
      :on-drag (lambda (x y region)
        (if gesture.start
          (eseq.effects.mnm-surface/mnm-write scope "dcw" (+ (nth gesture.start 1) (* 0.5 (- (nth gesture.start 0) y)))) false)))))
(def fcz-waves '("Saw" "Square" "Pulse" "Double sine" "Saw-pulse" "Reso saw" "Reso triangle" "Reso trapezoid"))
(def fcz-wave-page ()
  (v-stack :gap 0.25
    (fcz-wave)
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-option "wave" fcz-waves)
      (eseq.effects.mnm-surface/mnm-option "wave2" (cons "Wave2 off" fcz-waves)))
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "dcw" "DCW" 2)
      (eseq.effects.mnm-surface/mnm-num "env_amt" "Env > DCW" 2))))
(def fcz-line-page ()
  (v-stack :gap 0.3
    (eseq.effects.mnm-surface/mnm-option "line" '("Mix" "Ring"))
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "dtn" "DTN" 0)
      (eseq.effects.mnm-surface/mnm-num "sub" "Sub" 2))))
(def fcz-amp-page ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.15
      (adsr-editor :debug-name "fcz-amp" :width 35.2 :height 3.2
        :curve-color :black :point-color :black :grid-color :black
        :background-color (eseq.effects.mnm-surface/mnm-accent)
        :attack (eseq.effects.mnm-surface/mnm-bind "amp_attack_ms")
        :decay (eseq.effects.mnm-surface/mnm-bind "amp_decay_ms")
        :sustain (eseq.effects.mnm-surface/mnm-bind "amp_sustain")
        :release (eseq.effects.mnm-surface/mnm-bind "amp_release_ms")
        :attack-max 10000 :decay-max 10000 :release-max 10000
        :on-change (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" env)))
      (h-stack :gap 0.3
        (eseq.effects.mnm-surface/mnm-num "amp_attack_ms" "Attack" 1)
        (eseq.effects.mnm-surface/mnm-num "amp_decay_ms" "Decay" 1)
        (eseq.effects.mnm-surface/mnm-num "amp_sustain" "Sustain" 2)
        (eseq.effects.mnm-surface/mnm-num "amp_release_ms" "Release" 1)))))
(def fcz-out-page ()
  (v-stack :gap 0.3
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "gain" "Level" 2))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 0 "PHASE" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 0 "dcw" "DCW" 10.3 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "env_amt" "Env > DCW" 10.3 2 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 1 "LINE" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 1 "dtn" "DTN" 10.3 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 1 "sub" "Sub" 10.3 2 :linear))))
    (eseq.effects.mnm-surface/mnm-display "FELUCCA CZ" '("PHASE" "LINE" "ENVELOPE" "OUT")
      (list fcz-wave-page fcz-line-page fcz-amp-page fcz-out-page))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 2 "ENVELOPE" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 2 "amp_attack_ms" "Attack" 7 0 :log)
          (eseq.effects.mnm-surface/mnm-knob 2 "amp_release_ms" "Release" 7 0 :log)))
      (eseq.effects.mnm-surface/mnm-panel 3 "OUT" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 3 "gain" "Level" 14.2 2 :linear))))))
