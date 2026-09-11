;; Vox: glottal pulse/breath excitation through four moving formants.
(def vox-source ()
  (v-stack :gap 0.3
    (eseq.effects.mnm-surface/mnm-source-preview 2 "glottis")
    (eseq.effects.mnm-surface/mnm-caption "Glottal pulse before breath mixing and formant filtering.")
    (h-stack :gap 0.3 (eseq.effects.mnm-surface/mnm-num "growl_hz" "Growl Hz" 1))))
(def vox-frequency (values)
  (let ((v (floor (eseq.effects.mnm-surface/mnm-value "vowel"))) (m (eseq.effects.mnm-surface/mnm-value "vowel_morph")))
    (* (+ (* (nth values v) (- 1 m)) (* (nth values (if (>= v 9) 0 (+ v 1))) m))
      (eseq.effects.mnm-surface/mnm-value "formant_shift"))))
(defwidget vox-formants
  :width 35.2 :height 4.2 :state (f1 f2 f3 f4 q) :bindable (f1 f2 f3 f4 q)
  :shader
  (let ((hz (max 1 (* 9000 (pow (/ (+ (/ x aspect) 1) 2) 2))))
        (r1 (/ hz (clamp f1 60 9000))) (r2 (/ hz (clamp f2 60 9000)))
        (r3 (/ hz (clamp f3 60 9000))) (r4 (/ hz (clamp f4 60 9000)))
        (response (/ (+ (/ 1 (sqrt (+ 1 (pow (* q (- r1 (/ 1 r1))) 2))))
                        (/ 0.55 (sqrt (+ 1 (pow (* q (- r2 (/ 1 r2))) 2))))
                        (/ 0.28 (sqrt (+ 1 (pow (* q (- r3 (/ 1 r3))) 2))))
                        (/ 0.15 (sqrt (+ 1 (pow (* q (- r4 (/ 1 r4))) 2))))) 1.98))
        (curve (- 0.8 (* 2.6 response))))
    (sdf/layer
      (sdf/region :formants (sdf/rect width height) :yellow)
      (sdf/paint (max (- y 0.8) (- curve y)) :black)
      (sdf/paint (sdf/translate 0 0.8 (sdf/rect aspect 0.008)) (rgba 0 0 0 0.3)))))
(def vox-vowel ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (v-stack :gap 0.25
      (vox-formants :debug-name "vox-formants"
        :f1 (vox-frequency '(650 660 400 290 270 520 490 400 570 350))
        :f2 (vox-frequency '(1080 1720 1700 1870 2140 1190 1350 800 840 600))
        :f3 (vox-frequency '(2650 2410 2600 2800 2950 2390 1690 2600 2410 2700))
        :f4 (vox-frequency '(2900 2750 3000 3250 3300 2900 2700 2800 2700 2900))
        :q (eseq.effects.mnm-surface/mnm-bind "formant_q")
        :on-mouse-down (lambda (x y region)
          (set! gesture.start (list x y (eseq.effects.mnm-surface/mnm-value "formant_shift") (eseq.effects.mnm-surface/mnm-value "formant_q"))))
        :on-mouse-up (lambda (x y region) (set! gesture.start nil))
        :on-drag (lambda (x y region)
          (if gesture.start
            (do (eseq.effects.mnm-surface/mnm-write scope "formant_shift" (* (nth gesture.start 2) (exp (- x (nth gesture.start 0)))))
                (eseq.effects.mnm-surface/mnm-write scope "formant_q" (+ (nth gesture.start 3) (* 8 (- (nth gesture.start 1) y)))))
            false)))
      (eseq.effects.mnm-surface/mnm-caption "Nominal formant bank / 0–9 kHz, square-root axis.")
      (eseq.effects.mnm-surface/mnm-option "vowel" '("A" "AE" "E" "I" "EE" "UH" "ER" "O" "AW" "OO"))
      (h-stack :gap 0.3 (eseq.effects.mnm-surface/mnm-num "formant_q" "Focus Q" 1)))))
(def vox-consonant ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (scale (nth '(1 1 1 1 0.3 0.3 0.25 1.4) (floor (eseq.effects.mnm-surface/mnm-value "cons_type")))))
    (v-stack :gap 0.3
      (adsr-editor :debug-name "vox-consonant-envelope" :mode :decay :width 35.2 :height 3.6
        :initial 1 :initial-min 0 :initial-max 1 :initial-editable false :decay-db 60
        :time (* scale (eseq.effects.mnm-surface/mnm-value "cons_len_ms")) :time-max (* scale 400)
        :curve-color :black :point-color :black :grid-color :black :background-color (eseq.effects.mnm-surface/mnm-accent)
        :on-change (lambda (env) (eseq.effects.mnm-surface/mnm-write scope "cons_len_ms" (/ (get env :time) scale))))
      (eseq.effects.mnm-surface/mnm-option "cons_type" '("None" "S" "SH" "F" "T" "K" "P" "H"))
      (h-stack :gap 0.3 (eseq.effects.mnm-surface/mnm-num "sibilance" "Sibilance" 2) (eseq.effects.mnm-surface/mnm-num "cons_duck" "Vowel Duck" 2))
      (eseq.effects.mnm-surface/mnm-caption "Consonant decay includes the selected articulation's length scale."))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 0 "VOICE" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 0 "glottis" "Glottis" 6.8 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "breath" "Breath" 6.8 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "growl" "Growl" 6.8 2 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 1 "VOWEL" 21.4
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 1 "vowel_morph" "Morph" 10.3 2 :linear) (eseq.effects.mnm-surface/mnm-knob 1 "formant_shift" "Shift" 10.3 2 :log))))
    (eseq.effects.mnm-surface/mnm-display "VOX" '("VOICE" "VOWEL" "FILTER" "AMPLITUDE" "CONSONANT" "COLOR")
      (list vox-source vox-vowel eseq.effects.mnm-surface/mnm-filter-page eseq.effects.mnm-surface/mnm-amp-page vox-consonant eseq.effects.mnm-surface/mnm-color-page))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 2 "FILTER" 14.6
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 2 "flt_base" "Base Hz" 7 0 :log) (eseq.effects.mnm-surface/mnm-knob 2 "flt_width" "Width oct" 7 2 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 3 "AMP" 14.6
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 3 "gain" "Level" 7 2 :linear) (eseq.effects.mnm-surface/mnm-knob 3 "pan_width" "Spread" 7 2 :linear))))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 4 "CONSONANT" 14.6
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 4 "cons_level" "Level" 7 2 :linear) (eseq.effects.mnm-surface/mnm-knob 4 "cons_len_ms" "Length ms" 7 1 :log)))
      (eseq.effects.mnm-surface/mnm-panel 5 "COLOR" 14.6
        (h-stack :gap 0.2 (eseq.effects.mnm-surface/mnm-knob 5 "drive" "Drive" 7 2 :linear) (eseq.effects.mnm-surface/mnm-knob 5 "srr" "Reduction" 7 2 :linear))))))
