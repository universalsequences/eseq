(def stroke-row (text)
  (label text :width 11.6 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def strokes-view ()
  (v-stack :gap 0.15
    (tabla-caption "Strokes repeat every octave / C4 octave = recorded pitch")
    (h-stack :gap 0.2
          (v-stack :gap 0.05 (stroke-row "C  Ge Meend") (stroke-row "C#  Ge Rise") (stroke-row "D  Dha") (stroke-row "D#  Dha High"))
          (v-stack :gap 0.05 (stroke-row "E  Tun") (stroke-row "F  Te") (stroke-row "F#  Ti") (stroke-row "G  Na Crack"))
          (v-stack :gap 0.05 (stroke-row "G#  Roll Open") (stroke-row "A  Roll Finger") (stroke-row "A#  Roll Tin") (stroke-row "B  Roll Bright")))))

(def tabla-bind (name) (eseq.effects.physical-model-surface/bind name))

(def tabla-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

;; Normalized two-pole contact force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-tabla-hand
  :width 35.3 :height 3.1 :state (hardness dynamics)
  :shader
  (let ((time (* 0.0015 0.5 (+ 1 (/ x aspect))))
        (hard (* 0.00025 (pow 2 (* 3 (- 0.5 hardness)))))
        (soft (* hard (+ 1 (* 0.75 dynamics))))
        (u (/ time hard)) (v (/ time soft))
        (curve (- 0.8 (* 1.5 u (pow 2.7182818 (- 1 u)))))
        (quiet (- 0.8 (* 1.5 (/ hard soft) v (pow 2.7182818 (- 1 v))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y quiet)) 0.018) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def hand-view ()
  (v-stack :gap 0.15 (tabla-caption "Hand contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-tabla-hand :debug-name "pm-tabla-hand" :hardness (tabla-bind "hand.hardness")
      :dynamics (tabla-bind "hand.dynamics"))))

;; Bayan ring with decay and muffle loss (measured 2.48/s), and the
;; meend: pitch rising at the measured 578.4 cents/s for 0.187 s.
(defwidget pm-tabla-bayan
  :width 35.3 :height 3.1 :state (decay muffle meend press)
  :shader
  (let ((time (* 0.6 0.5 (+ 1 (/ x aspect))))
        (rate (+ (/ 2.48 decay) (* 90 muffle muffle)))
        (curve (- 0.8 (* 1.55 (pow 2.7182818 (* -1 rate time)))))
        (cents (+ (* 100 press) (* meend 578.4 (min time 0.187))))
        (bend (- 0.35 (* 0.0012 cents))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y bend)) 0.015) (rgba 0.16 0.06 0.11 0.45))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def bayan-view ()
  (v-stack :gap 0.15 (tabla-caption "Bayan ring / 0-0.6 s / faint = meend pitch rise")
    (pm-tabla-bayan :debug-name "pm-tabla-bayan" :decay (tabla-bind "head.decay")
      :muffle (tabla-bind "head.muffle") :meend (tabla-bind "bayan.meend") :press (tabla-bind "bayan.press"))))

;; Skin noise band: level and centre shift around a 2 kHz reference.
(defwidget pm-tabla-skin
  :width 35.3 :height 3.1 :state (skin tone)
  :shader
  (let ((octave (* 4 (/ x aspect)))
        (centre (* 1.5 tone))
        (d (- octave centre))
        (curve (- 0.8 (* 0.52 skin (/ 1 (+ 1 (* 1.6 d d)))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def skin-view ()
  (v-stack :gap 0.15 (tabla-caption "Skin contact noise / 125 Hz-32 kHz, log / band per stroke")
    (pm-tabla-skin :debug-name "pm-tabla-skin" :skin (tabla-bind "contact.skin")
      :tone (tabla-bind "contact.skin_tone"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM TABLA"
    (list
      '("HAND" ("hand.hardness" "Hardness" 2 :linear) ("hand.flam" "Flam" 2 :linear))
      '("HEADS" ("head.decay" "Decay" 2 :log) ("head.muffle" "Muffle" 2 :linear))
      '("BAYAN" ("bayan.meend" "Meend" 2 :linear) ("bayan.press" "Press st" 1 :linear))
      '("TUNE & LEVEL" ("head.tune" "Tune st" 1 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Strokes"
        :view (lambda () (strokes-view))
        :controls (lambda () '(("hand.flam" "Flam spacing" 2) ("hand.flam_level" "Flam level" 2)))
        :hint "Flam scales the measured contact spacing.")
      (dict :title "Hand"
        :view (lambda () (hand-view))
        :controls (lambda () '(("hand.hardness" "Hardness" 2) ("hand.dynamics" "Vel > timbre" 2)))
        :hint "Harder hands shorten contact; energy is redistributed, not added.")
      (dict :title "Bayan"
        :view (lambda () (bayan-view))
        :controls (lambda () '(("head.muffle" "Muffle" 2) ("bayan.press" "Press st" 1)))
        :hint "Meend replays the wrist's pitch rise; Press shifts only the bayan.")
      (dict :title "Skin"
        :view (lambda () (skin-view))
        :controls (lambda () '(("contact.skin" "Skin noise" 2) ("contact.skin_tone" "Skin tone" 2)))
        :hint "Hand-on-skin contact noise, band-limited per stroke.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("contact.skin" "Skin" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0)))
        :hint "The reference is mono; both channels carry the same signal."))))
