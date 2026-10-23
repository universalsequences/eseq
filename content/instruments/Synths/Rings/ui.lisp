;; Rings — the module's front panel: the resonator model and its four
;; controls (with what STRUCTURE / BRIGHTNESS / DAMPING / POSITION do in the
;; selected model, and the chord name in the chord models), the voice strip,
;; and the ODD / EVEN output spread. Every control appears exactly once.

(def rg-knob () (eseq.effects.custom-ui-lego/ui-accent-violet))
(def rg-head () (eseq.effects.custom-ui-lego/ui-accent-violet))
(def rg-text () :fg)

(def rg-surf () :instrument-group-bg)

;; A flat section panel like the 808 Kick's: one thin padding, so the section
;; badge sits tight in the top-left corner. Click selects the section.
(def rg-panel (section width height body)
  (box :width width :height height :padding 0.18
    :background-color (rg-surf)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))

(def rg-panel-dense (section body)
  (rg-panel section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) body))
(def rg-small-row (body)
  (box :width :fill :height :fill :v-align :start body))

;; The six resonator models, then Disastrous Peace with each of its effects.
(def rg-model-options ()
  '("Modal" "Sympathetic" "Inharmonic" "FM voice" "Sym. chords" "String+verb"
    "Peace formant" "Peace chorus" "Peace reverb" "Peace formant 2" "Peace ensemble" "Peace reverb 2"))

;; Per model: STRUCTURE, BRIGHTNESS, DAMPING, POSITION.
(def rg-model-help ()
  '(("material" "brightness" "decay" "strike point")
    ("string intervals" "brightness" "decay" "pluck point")
    ("stiffness, bridge" "brightness" "decay" "pluck point")
    ("frequency ratio" "FM index" "decay, max holds" "feedback -/+")
    ("chord" "brightness" "decay" "pluck point")
    ("stiffness, bridge" "bright, verb tone" "decay, verb size" "pluck, width")
    ("chord" "registration" "decay > attack" "vowel")
    ("chord" "registration" "decay > attack" "chorus")
    ("chord" "registration" "decay > attack" "reverb")
    ("chord" "registration" "decay > attack" "vowel")
    ("chord" "registration" "decay > attack" "ensemble")
    ("chord" "registration" "decay > attack" "reverb")))

(def rg-chord-names ()
  '("octaves" "fifths" "sus4" "minor" "m7" "m9" "m11" "6/9" "maj9" "maj7" "major"))

(def rg-value (name fallback)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.custom-ui-runtime/custom-ui-param-value p) fallback)))

(def rg-model () (max 0 (min 11 (round (rg-value "model" 0)))))

;; STRUCTURE line: the chord name in the chord models.
(def rg-help-text (index)
  (let ((m (rg-model)))
    (if (and (= index 0) (or (= m 4) (>= m 6)))
      (str "chord " (nth (rg-chord-names) (max 0 (min 10 (round (* (rg-value "structure" 0.5) 10))))))
      (nth (nth (rg-model-help) m) index))))

(def rg-help-line (index prefix)
  (label (str prefix (rg-help-text index)) :font-size 10 :color :dim :bg :transparent))

(def rg-main-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def rg-main-w () (eseq.effects.custom-ui-lego/ui-lego-wide-col-w))

(def rg-main-block ()
  (rg-panel 0 (rg-main-w) (rg-main-h)
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 0 "RINGS" 4.4 (rg-head))
      (h-stack :width :fill :gap 0.40 :align :start
        (h-stack :gap 0.22 :align :center
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "structure" "structure" 4.6 4.2 3.6 (rg-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "brightness" "bright" 4.6 4.2 3.6 (rg-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "damping" "damping" 4.6 4.2 3.6 (rg-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "position" "position" 4.6 4.2 3.6 (rg-knob) 2))
        (v-stack :gap 0.10 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "model" "model" 10.4 (rg-model-options) (rg-text))
          (rg-help-line 0 "S: ")
          (rg-help-line 1 "B: ")
          (rg-help-line 2 "D: ")
          (rg-help-line 3 "P: "))))))

(def rg-voice-block ()
  (rg-panel 3 (rg-main-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    (rg-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 3 "VOICE" 4.0 (rg-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 3 5.0 (rg-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "tune" "tune" 5.0 1 "st" (rg-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "level_db" "level" 5.0 1 "dB" (rg-text))))))

;; ODD / EVEN outputs: partials or pickups split across the stereo field.
(def rg-out-block ()
  (rg-panel-dense 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :start
      (v-stack :width 10.2 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 1 "OUTPUT" 4.4 (rg-head))
        (label "odd L, even R" :font-size 9.5 :color :dim :bg :transparent)
        (label "0: mono sum" :font-size 9.5 :color :dim :bg :transparent))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "spread" "spread" 3.7 (rg-knob) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (v-stack :width (rg-main-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (rg-main-block)
      (rg-voice-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (rg-out-block))))
