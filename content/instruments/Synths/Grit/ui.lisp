;; Grit: two SID-like oscillators, with sync/ring, interlace and bit reduction.
(def grit-source ()
  (v-stack :gap 0.3
    (if (< (eseq.effects.mnm-surface/mnm-value "osc_wave") 4)
      (eseq.effects.mnm-surface/mnm-source-preview (eseq.effects.mnm-surface/mnm-bind "osc_wave") "pw")
      (box :width 35.2 :height 4.2
        (v-stack :gap 0.4
          (eseq.effects.mnm-surface/mnm-caption "Keytracked sample-and-hold noise")
          (eseq.effects.mnm-surface/mnm-caption "The noise clock follows the note at eight times its frequency."))))
    (eseq.effects.mnm-surface/mnm-option "osc_wave" '("Triangle" "Saw" "Pulse" "Saw x Pulse" "Clocked noise"))))
(defwidget grit-interlace-view
  :width 35.2 :height 3.5 :state (rate depth) :bindable (rate depth)
  :shader
  (let ((u (/ (+ (/ x aspect) 1) 2))
        (level (if (< (fract (* u rate 0.25)) 0.5) 1 (- 1 depth)))
        (line (- 0.75 (* 1.5 level))))
    (sdf/layer
      (sdf/region :interlace (sdf/rect width height) :yellow)
      (sdf/paint (- (abs (- y line)) 0.025) :black))))
(def grit-interlace ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (grit-interlace-view :debug-name "grit-interlace"
      :rate (eseq.effects.mnm-surface/mnm-bind "interlace_hz")
      :depth (eseq.effects.mnm-surface/mnm-bind "interlace")
      :on-mouse-down (lambda (x y region)
        (set! gesture.start (list x y (eseq.effects.mnm-surface/mnm-value "interlace_hz")
          (eseq.effects.mnm-surface/mnm-value "interlace"))))
      :on-mouse-up (lambda (x y region) (set! gesture.start nil))
      :on-drag (lambda (x y region)
        (if gesture.start
          (do (eseq.effects.mnm-surface/mnm-write scope "interlace_hz" (* (nth gesture.start 2) (exp (- x (nth gesture.start 0)))))
              (eseq.effects.mnm-surface/mnm-write scope "interlace" (+ (nth gesture.start 3) (* 0.5 (- (nth gesture.start 1) y)))))
          false)))))
(def grit-motion ()
  (v-stack :gap 0.3
    (grit-interlace)
    (eseq.effects.mnm-surface/mnm-option "osc_mode" '("Free" "Sync" "Ring" "Sync + Ring"))
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "fine_cents" "Fine cents" 1)
      (eseq.effects.mnm-surface/mnm-num "interlace_hz" "Interlace Hz" 1))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 0 "SOURCE" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 0 "pw" "Pulse Width" 6.8 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "bits" "Bits" 6.8 1 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "tune_semi" "OSC 2 st" 6.8 0 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 1 "INTERACTION" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 1 "osc2_level" "OSC 2 Level" 10.3 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 1 "interlace" "Interlace" 10.3 2 :linear))))
    (eseq.effects.mnm-surface/mnm-display "GRIT" '("SOURCE" "INTERACTION" "FILTER" "AMPLITUDE" "COLOR")
      (list grit-source grit-motion eseq.effects.mnm-surface/mnm-filter-page eseq.effects.mnm-surface/mnm-amp-page eseq.effects.mnm-surface/mnm-color-page))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 2 "FILTER" 14.6
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 2 "flt_base" "Base Hz" 7 0 :log) (eseq.effects.mnm-surface/mnm-knob 2 "flt_width" "Width oct" 7 2 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 3 "AMP" 14.6
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 3 "gain" "Level" 7 2 :linear) (eseq.effects.mnm-surface/mnm-knob 3 "pan_width" "Spread" 7 2 :linear))))
    (box :width 9.2 :height 9.8 :padding 0.2
      :background-color (if (= (eseq.effects.mnm-surface/mnm-section) 4) :instrument-panel-bg :instrument-group-bg)
      :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback 4)
      (v-stack :gap 0.3
        (box :width 7 :height 0.7 :background-color (eseq.effects.mnm-surface/mnm-accent)
          (label "COLOR" :width 7 :height 0.7 :font-size 8 :h-align :center :v-align :center :color :black :bg :transparent))
        (eseq.effects.mnm-surface/mnm-knob 4 "drive" "Drive" 8.8 2 :linear)
        (eseq.effects.mnm-surface/mnm-knob 4 "srr" "Reduction" 8.8 2 :linear)))))
