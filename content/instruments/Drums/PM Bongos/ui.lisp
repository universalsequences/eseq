(def stroke-row (text)
  (label text :width 11.6 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def strokes-view ()
  (v-stack :gap 0.15
    (bongo-caption "Strokes repeat every octave / C4 octave = recorded pitch")
    (h-stack :gap 0.2
          (v-stack :gap 0.05 (stroke-row "C  Low Open") (stroke-row "C#  Slap + Ghost") (stroke-row "D  Low Ghost") (stroke-row "D#  Pressed Tone"))
          (v-stack :gap 0.05 (stroke-row "E  Mid Open Flam") (stroke-row "F  Mid Open") (stroke-row "F#  Mid Open Short") (stroke-row "G  Mid Ghost"))
          (v-stack :gap 0.05 (stroke-row "G#  Low Open 2") (stroke-row "A  Slap Low Flam") (stroke-row "A#  High Low Muted") (stroke-row "B  High Slap Flam")))))

(def bongo-bind (name) (eseq.effects.physical-model-surface/bind name))

(def bongo-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

;; Normalized two-pole contact force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-bongo-hand
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
  (v-stack :gap 0.15 (bongo-caption "Hand contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-bongo-hand :debug-name "pm-bongo-hand" :hardness (bongo-bind "hand.hardness")
      :dynamics (bongo-bind "hand.dynamics"))))

;; Low-head fundamental ring (measured 11.5/s) with decay and muffle loss.
(defwidget pm-bongo-head
  :width 35.3 :height 3.1 :state (decay muffle glide)
  :shader
  (let ((time (* 0.6 0.5 (+ 1 (/ x aspect))))
        (rate (+ (/ 11.5 decay) (* 90 muffle muffle)))
        (curve (- 0.8 (* 1.55 (pow 2.7182818 (* -1 rate time)))))
        (bend (- 0.75 (* 12 0.03 glide (pow 2.7182818 (/ time -0.028))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y bend)) 0.015) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def head-view ()
  (v-stack :gap 0.15 (bongo-caption "Low head ring / 0-0.6 s / faint = tension glide")
    (pm-bongo-head :debug-name "pm-bongo-head" :decay (bongo-bind "head.decay")
      :muffle (bongo-bind "head.muffle") :glide (bongo-bind "head.glide"))))

;; Skin noise band: level and centre shift around a 2 kHz reference.
(defwidget pm-bongo-skin
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
  (v-stack :gap 0.15 (bongo-caption "Skin contact noise / 125 Hz-32 kHz, log / band per stroke")
    (pm-bongo-skin :debug-name "pm-bongo-skin" :skin (bongo-bind "contact.skin")
      :tone (bongo-bind "contact.skin_tone"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM BONGOS"
    (list
      '("HAND" ("hand.hardness" "Hardness" 2 :linear) ("hand.flam" "Flam" 2 :linear))
      '("HEADS" ("head.decay" "Decay" 2 :log) ("head.muffle" "Muffle" 2 :linear))
      '("PITCH" ("head.tune" "Tune st" 1 :linear) ("head.glide" "Glide" 2 :linear))
      '("SKIN & LEVEL" ("contact.skin" "Skin" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Strokes"
        :view (lambda () (strokes-view))
        :controls (lambda () '(("hand.flam" "Flam spacing" 2) ("hand.flam_level" "Flam level" 2)))
        :hint "Flam scales the measured contact spacing.")
      (dict :title "Hand"
        :view (lambda () (hand-view))
        :controls (lambda () '(("hand.hardness" "Hardness" 2) ("hand.dynamics" "Vel > timbre" 2)))
        :hint "Harder hands shorten contact; energy is redistributed, not added.")
      (dict :title "Heads"
        :view (lambda () (head-view))
        :controls (lambda () '(("head.muffle" "Muffle" 2) ("head.glide" "Tension glide" 2)))
        :hint "Muffle rests a hand on the head. Glide bends hard hits.")
      (dict :title "Skin"
        :view (lambda () (skin-view))
        :controls (lambda () '(("contact.skin" "Skin noise" 2) ("contact.skin_tone" "Skin tone" 2)))
        :hint "Hand-on-skin contact noise, band-limited per stroke.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0)))
        :hint "Stereo 1 reproduces the measured head placement."))))
