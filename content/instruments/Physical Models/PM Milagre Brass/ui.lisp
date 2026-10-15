;; PM Milagre Brass: the shared physical-model surface with a lip-valve view.
(def mb-bind (name) (eseq.effects.physical-model-surface/bind name))

(def mb-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

;; Lip opening over two buzz cycles at full breath, as the DSP computes it:
;; max(0, lips + buzz sin). Faint = half breath (excursion scales by 0.5^curve).
(defwidget pm-brass-lips
  :width 35.3 :height 3.1 :state (lips buzz curve)
  :shader
  (let ((wave (sin (* 6.2831853 (+ 1 (/ x aspect)))))
        (scale (+ (abs lips) buzz 0.01))
        (full (max 0 (+ lips (* buzz wave))))
        (half (max 0 (+ lips (* buzz (pow 0.5 curve) wave))))
        (line (- 0.8 (* 1.5 (/ full scale))))
        (soft (- 0.8 (* 1.5 (/ half scale)))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- line y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y soft)) 0.018) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y line)) 0.025) (rgba 0.16 0.06 0.11 1)))))

(def lips-view ()
  (v-stack :gap 0.15 (mb-caption "Lip opening")
    (pm-brass-lips :debug-name "pm-brass-lips" :lips (mb-bind "lips.lips")
      :buzz (mb-bind "lips.buzz") :curve (mb-bind "lips.buzz_curve"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM MILAGRE BRASS"
    (list
      '("BREATH" ("blow.breath" "Breath" 2 :linear) ("blow.pressure" "Pressure" 2 :linear))
      '("LIPS" ("lips.buzz" "Buzz" 2 :linear) ("lips.lips" "Lips" 2 :linear))
      '("HORN" ("bore.horn" "Horn key" 0 :linear) ("bore.bell_hz" "Bell" 0 :log))
      '("ROOM" ("room.body" "Body" 2 :linear) ("room.hall" "Hall" 2 :linear)))
    (list
      (dict :title "Lips"
        :view (lambda () (lips-view))
        :controls (lambda () '(("lips.buzz_curve" "Breath > buzz" 2) ("lips.register" "Register" 2)
          ("blow.air" "Air noise" 3))))
      (dict :title "Breath"
        :view (lambda () (box))
        :controls (lambda () '(("blow.breath_ms" "Breath glide" 0) ("blow.vel_breath" "Velocity > breath" 2)
          ("blow.reg_breath" "Register breath" 2) ("voice_mode" "Voicing" 0))))
      (dict :title "Horn"
        :view (lambda () (box))
        :controls (lambda () '(("bore.impedance" "Impedance" 2) ("bore.bore_q" "Bore Q" 2))))
      (dict :title "Pitch"
        :view (lambda () (box))
        :controls (lambda () '(("pitch.tune" "Tune" 1) ("pitch.slide" "Slide" 0)
          ("pitch.vib_cent" "Vibrato depth" 1) ("pitch.vib_hz" "Vibrato rate" 2))))
      (dict :title "Room"
        :view (lambda () (box))
        :controls (lambda () '(("room.hall_s" "Hall time" 2) ("gain" "Output" 2))))
      (dict :title "Amp"
        :view (lambda () (eseq.effects.physical-model-surface/envelope 5))
        :controls (lambda () '(("amp.attack" "Attack" 1) ("amp.decay" "Decay" 1) ("amp.sustain" "Sustain" 2)
          ("amp.release" "Release" 1) ("gain" "Output" 2)))))))
