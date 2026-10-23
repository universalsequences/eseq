;; Elements — the module's front panel as five surfaces: the resonator (model,
;; geometry / brightness / damping / position, with what they do in the
;; selected model) and the voice strip on the left; the three exciters (bow
;; with the contour envelope, blow, strike) and SPACE on the right. Every
;; control appears exactly once.

(def el-knob () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def el-head () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def el-text () :fg)

(def el-surf () :instrument-group-bg)

;; A flat section panel like the 808 Kick's: one thin padding, so the section
;; badge sits tight in the top-left corner. Click selects the section.
(def el-panel (section width height body)
  (box :width width :height height :padding 0.18
    :background-color (el-surf)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))

(def el-panel-dense (section body)
  (el-panel section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) body))
(def el-small-row (body)
  (box :width :fill :height :fill :v-align :start body))

(def el-value (name fallback)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.custom-ui-runtime/custom-ui-param-value p) fallback)))

(def el-model () (max 0 (min 3 (round (el-value "model" 0)))))

;; The STRINGS model's chords (voice.cc `chords`, picked by GEOMETRY).
(def el-chords ()
  '("octaves" "minor 7" "minor" "minor add9" "minor 11" "fifths"
    "major 11" "major add9" "major" "major 7" "sus4"))

;; Per model: GEOMETRY, BRIGHTNESS, DAMPING, POSITION.
(def el-model-help ()
  '(("geo: plate > bell" "bright: high modes" "damp: decay" "pos: pickup")
    ("geo: bridge > disp." "bright: loop LP" "damp: decay" "pos: pickup")
    ("chord" "bright: loop LP" "damp: decay" "pos: pickup")
    ("blow/strike: osc 1/2" "bow: detune / fb" "geo: filter, bright: cut" "damp: env, pos: spin")))

(def el-help-text (index)
  (let ((m (el-model)))
    (if (and (= index 0) (= m 2))
      (str "chord: " (nth (el-chords) (max 0 (min 10 (round (* (el-value "geometry" 0.2) 10))))))
      (nth (nth (el-model-help) m) index))))

(def el-help-line (index)
  (label (el-help-text index) :font-size 8.5 :color :dim :bg :transparent))

(def el-main-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def el-main-w () (eseq.effects.custom-ui-lego/ui-lego-wide-col-w))

(def el-main-block ()
  (el-panel 0 (el-main-w) (el-main-h)
    (v-stack :width :fill :height :fill :gap 0.26 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 0 "ELEMENTS" 5.4 (el-head))
      (h-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.10 :align :center
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "geometry" "geometry" 4.9 4.2 3.6 (el-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "brightness" "bright" 4.9 4.2 3.6 (el-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "damping" "damping" 4.9 4.2 3.6 (el-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "position" "position" 4.9 4.2 3.6 (el-knob) 2))
        (v-stack :gap 0.10 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "model" "model" 8.4 '("Modal" "String" "Strings" "Ominous") (el-text))
          (el-help-line 0)
          (el-help-line 1)
          (el-help-line 2)
          (el-help-line 3))))))

(def el-voice-block ()
  (el-panel 5 (el-main-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    (el-small-row
      (h-stack :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 5 "VOICE" 3.6 (el-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 5 4.4 (el-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "tune" "tune" 4.4 1 "st" (el-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "strength" "strength" 4.4 2 false (el-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "vel_amt" "vel" 4.0 2 false (el-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "level_db" "level" 4.4 1 "dB" (el-text))))))

(def el-exciter (section title note a a-title b b-title c c-title)
  (el-panel-dense section
    (h-stack :width :fill :height :fill :gap 0.30 :align :start
      (v-stack :width 9.0 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-section-badge-s section title 4.4 (el-head))
        (label note :font-size 9 :color :dim :bg :transparent))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s section a a-title 3.9 (el-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s section b b-title 3.9 (el-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s section c c-title 3.9 (el-knob) 2)))))

(def el-bow-block ()
  (el-exciter 1 "BOW" "contour: envelope" "contour" "contour" "bow" "level" "bow_timbre" "timbre"))
(def el-blow-block ()
  (el-exciter 2 "BLOW" "level > 2/3: tube" "blow" "level" "flow" "flow" "blow_timbre" "timbre"))
(def el-strike-block ()
  (el-exciter 3 "STRIKE" "samples > dust" "strike" "level" "mallet" "mallet" "strike_timbre" "timbre"))

(def el-space-block ()
  (el-panel-dense 4
    (h-stack :width :fill :height :fill :gap 0.30 :align :start
      (v-stack :width 9.0 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 4 "SPACE" 4.4 (el-head))
        (h-stack :gap 0.30 :align :center
          (label "freeze" :font-size 9.0 :width 3.0 :color (el-text) :bg :transparent)
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 4 "freeze" "freeze" 4.6 '("off" "on") (el-text))))
      (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 4 "space" "space" 3.9 (el-knob) 2))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (v-stack :width (el-main-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (el-main-block)
      (el-voice-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (el-bow-block)
      (el-blow-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (el-strike-block)
      (el-space-block))))
